//! Qualification of the source-owned MAC slice replacing `wDev_ProcessFiq`.
//!
//! The vendor root drains MAC, BSS-color, and Wi-Fi power interrupt domains in
//! one loop. This first vertical slice deliberately enables only the ordinary
//! MAC domain. BSS-color and power interrupts are explicit initialization
//! prerequisites; later adapters must qualify them before those domains can be
//! enabled. The Rust probe executes production `handle_mac_irq` and its pure
//! ordered-work selector, while the linked vendor image executes the original
//! root with reviewed call-boundary results.

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
};

use open_esp_radio_esp32s31_wifi_lmac::irq::{
    HANDLED_MAC_MASK, MAC_INT_COLLISION, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK, MAC_INT_RX_SUCCESS,
    MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT,
};
use open_radio_vendor_validator_semantic::{
    DriverAdapterQualification, EffectDisposition, EffectPolicy, EffectSelector,
};

use crate::{MmioRegisterMap, Result, artifact_sha256, execution};

use super::{code_closure_sha256, inventory_symbol_sha256};

const MAC_STATUS: u32 = 0x2010_4c48;
const MAC_CLEAR: u32 = 0x2010_4c4c;

const INFRASTRUCTURE_CALLS: &[&str] = &[
    "hal_mac_is_dma_enable",
    "hal_mac_interrupt_get_event",
    "hal_mac_interrupt_get_bsscolor",
    "hal_pwr_interrupt_get_event",
    "pwr_hal_get_intr_raw_signal",
];
const CLEAR_CALLS: &[&str] = &[
    "hal_mac_interrupt_clr_event",
    "hal_mac_interrupt_clr_bsscolor",
    "hal_pwr_interrupt_clr_event",
    "hal_pwr_interrupt_clr_event",
];
const WORK_CALLS: &[(u32, &str, u32)] = &[
    (MAC_INT_RX_SUCCESS, "lmacProcessRxSucData", 1),
    (MAC_INT_TX_COMPLETE, "lmacPostTxComplete", 2),
    (MAC_INT_TX_TIMEOUT, "lmacProcessAllTxTimeout", 3),
    (MAC_INT_COLLISION, "lmacProcessCollisions", 4),
];

#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    status: u32,
}

const CASES: &[Case] = &[
    Case {
        name: "spurious",
        status: 0,
    },
    Case {
        name: "rx-success",
        status: MAC_INT_RX_SUCCESS,
    },
    Case {
        name: "tx-complete",
        status: MAC_INT_TX_COMPLETE,
    },
    Case {
        name: "tx-timeout",
        status: MAC_INT_TX_TIMEOUT,
    },
    Case {
        name: "collision",
        status: MAC_INT_COLLISION,
    },
    Case {
        name: "all-supported",
        status: HANDLED_MAC_MASK,
    },
    Case {
        name: "rx-with-observed-auxiliary",
        status: MAC_INT_RX_SUCCESS | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
    },
    Case {
        name: "observed-auxiliary-only",
        status: MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
    },
    Case {
        name: "unsupported-watchdog",
        status: 0x0000_0800,
    },
    Case {
        name: "supported-and-unsupported",
        status: HANDLED_MAC_MASK | 0x0000_0800,
    },
];

fn zeros(count: usize) -> VecDeque<u32> {
    std::iter::repeat_n(0, count).collect()
}

fn vendor_scenario(status: u32) -> execution::Scenario {
    let iterations = usize::from(status != 0) + 1;
    let mut call_returns = BTreeMap::from([
        ("hal_mac_is_dma_enable".to_owned(), zeros(iterations)),
        (
            "hal_mac_interrupt_get_bsscolor".to_owned(),
            zeros(iterations),
        ),
        ("hal_pwr_interrupt_get_event".to_owned(), zeros(iterations)),
        ("pwr_hal_get_intr_raw_signal".to_owned(), zeros(iterations)),
    ]);
    if status != 0 {
        call_returns.insert("hal_mac_interrupt_clr_bsscolor".to_owned(), zeros(1));
        call_returns.insert("hal_pwr_interrupt_clr_event".to_owned(), zeros(2));
        for (mask, symbol, _) in WORK_CALLS {
            if status & mask != 0 {
                call_returns.insert((*symbol).to_owned(), zeros(1));
            }
        }
    }
    let mut reads = VecDeque::from([status]);
    if status != 0 {
        reads.push_back(0);
    }
    execution::Scenario {
        mmio_reads: BTreeMap::from([(MAC_STATUS, reads)]),
        call_returns,
        max_steps: 2_000,
        ..execution::Scenario::default()
    }
}

fn rust_scenario(status: u32) -> execution::Scenario {
    let mut reads = VecDeque::from([status]);
    if status != 0 {
        reads.push_back(0);
    }
    execution::Scenario {
        mmio_reads: BTreeMap::from([(MAC_STATUS, reads)]),
        max_steps: 2_000,
        ..execution::Scenario::default()
    }
}

fn expected_calls(status: u32) -> Vec<&'static str> {
    let mut calls = INFRASTRUCTURE_CALLS.to_vec();
    if status != 0 {
        calls.extend_from_slice(CLEAR_CALLS);
        for (mask, symbol, _) in WORK_CALLS {
            if status & mask != 0 {
                calls.push(symbol);
            }
        }
        calls.extend_from_slice(INFRASTRUCTURE_CALLS);
    }
    calls
}

fn validate_vendor_calls(result: &execution::ExecutionResult, status: u32) -> Result<()> {
    let actual = result
        .ordered_calls
        .iter()
        .map(|call| call.symbol.as_str())
        .collect::<Vec<_>>();
    let expected = expected_calls(status);
    if actual != expected {
        return Err(format!(
            "wDev_ProcessFiq call order differs for status {status:#010x}: expected {expected:?}, got {actual:?}"
        )
        .into());
    }
    if status != 0 {
        let clear = result
            .ordered_calls
            .iter()
            .find(|call| call.symbol == "hal_mac_interrupt_clr_event")
            .expect("exact call order already requires the MAC clear call");
        if clear.arguments[0] != status {
            return Err(format!(
                "wDev_ProcessFiq clears {:#010x} instead of status {status:#010x}",
                clear.arguments[0]
            )
            .into());
        }
    }
    Ok(())
}

fn validate_vendor_mmio(result: &execution::ExecutionResult, status: u32) -> Result<()> {
    let expected = if status == 0 {
        vec![(false, MAC_STATUS, 0)]
    } else {
        vec![
            (false, MAC_STATUS, status),
            (true, MAC_CLEAR, status),
            (false, MAC_STATUS, 0),
        ]
    };
    let actual = result
        .events
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
                "unexpected wDev_ProcessFiq observable event for status {status:#010x}: {other:?}"
            )),
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if actual != expected {
        return Err(format!(
            "wDev_ProcessFiq MMIO differs for status {status:#010x}: expected {expected:?}, got {actual:?}"
        )
        .into());
    }
    Ok(())
}

fn rust_transaction_matches(events: &[execution::ExecutionEvent], status: u32) -> bool {
    match (status, events) {
        (
            0,
            [
                execution::ExecutionEvent::Read {
                    width: 32,
                    address: MAC_STATUS,
                    value: 0,
                    ..
                },
            ],
        ) => true,
        (
            status,
            [
                execution::ExecutionEvent::Read {
                    width: 32,
                    address: MAC_STATUS,
                    value: read_status,
                    ..
                },
                execution::ExecutionEvent::Write {
                    width: 32,
                    address: MAC_CLEAR,
                    value: cleared,
                    ..
                },
                execution::ExecutionEvent::Fence { .. },
                execution::ExecutionEvent::Read {
                    width: 32,
                    address: MAC_STATUS,
                    value: 0,
                    ..
                },
            ],
        ) if status != 0 && *read_status == status && *cleared == status => true,
        _ => false,
    }
}

fn validate_rust_transaction(result: &execution::ExecutionResult, status: u32) -> Result<()> {
    if !rust_transaction_matches(&result.events, status) {
        return Err(format!(
            "composed Rust IRQ transaction differs for status {status:#010x}: expected STATUS -> matching CLEAR -> fence -> drained STATUS, got {:?}",
            result.events
        )
        .into());
    }
    Ok(())
}

fn expected_semantic_encoding(status: u32) -> u32 {
    let handled = status & HANDLED_MAC_MASK;
    let disposition = if status == 0 {
        2
    } else if handled == 0 {
        3
    } else {
        1
    };
    let mut encoded = disposition | 1 << 7 | 1 << 31;
    if status != 0 {
        encoded |= 1 << 2;
    }
    let mut count = 0;
    for (mask, _, code) in WORK_CALLS {
        if status & mask != 0 {
            encoded |= code << (8 + count * 4);
            count += 1;
        }
    }
    encoded | count << 3
}

fn required_policy() -> BTreeMap<EffectSelector, EffectDisposition> {
    [
        (
            EffectSelector::MmioRead {
                width: 32,
                address: MAC_STATUS,
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::MmioWrite {
                width: 32,
                address: MAC_CLEAR,
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PlatformProvidedService {
                service: "embassy-mac-work-wakeup".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PublishedEvent {
                event: "mac-rx-success".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PublishedEvent {
                event: "mac-tx-complete".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PublishedEvent {
                event: "mac-tx-timeout".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PublishedEvent {
                event: "mac-collision".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::InitializationPrerequisite {
                prerequisite: "wifi-bss-color-interrupts-disabled".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::InitializationPrerequisite {
                prerequisite: "wifi-power-interrupts-disabled".to_owned(),
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
            "wDev_ProcessFiq MAC-slice effect policy differs from the closed adapter boundary:\nexpected {expected:#?}\nactual {actual:#?}"
        )
        .into());
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "qualification binds both immutable artifacts, one exact symbol and its policy"
)]
pub fn qualify_esp32s31_wdev_process_fiq_mac_slice(
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
        .ok_or("wDev_ProcessFiq qualification requires the caller-owned raw libpp inventory")?;
    if print_oracles {
        let vendor_archive_digest = artifact_sha256(vendor_inventory)?;
        let vendor_linked_digest = artifact_sha256(vendor_artifact)?;
        println!(
            "ORACLE\tlibpp\t{}\tsha256={vendor_archive_digest}",
            vendor_inventory.display()
        );
        println!(
            "ORACLE\tlibpp-linked\t{}\tsha256={vendor_linked_digest}",
            vendor_artifact.display()
        );
    }

    let mut vendor_image = execution::ExecutableImage::load(vendor_artifact)?;
    if let Some(companion) = vendor_companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = execution::ExecutableImage::load(rust_artifact)?;
    if let Some(companion) = rust_companion {
        rust_image.add_companion(companion)?;
    }

    let vendor_inventory_digest =
        inventory_symbol_sha256(vendor_inventory, None, "wDev_ProcessFiq")?;
    let vendor_code_digest = code_closure_sha256(&vendor_image, "wDev_ProcessFiq")?;
    let rust_code_digest = code_closure_sha256(&rust_image, rust_symbol)?;

    let mut canonical = String::from("driver-adapter esp32s31-wdev-process-fiq-mac-slice-v1\n");
    canonical.push_str(&format!(
        "vendor-inventory-symbol-sha256 {vendor_inventory_digest}\nvendor-linked-code-closure-sha256 {vendor_code_digest}\nrust-code-closure-sha256 {rust_code_digest}\n"
    ));
    canonical.push_str("scope mac-interrupt-domain\n");
    canonical.push_str("prerequisite wifi-bss-color-interrupts-disabled\n");
    canonical.push_str("prerequisite wifi-power-interrupts-disabled\n");
    canonical.push_str("pac-leaf hal_mac_interrupt_get_event\n");
    canonical.push_str("pac-leaf hal_mac_interrupt_clr_event\n");

    let mut matched = true;
    for case in CASES {
        let vendor = execution::execute(
            &vendor_image,
            svd,
            "wDev_ProcessFiq",
            vendor_scenario(case.status),
        )?;
        validate_vendor_calls(&vendor, case.status)?;
        validate_vendor_mmio(&vendor, case.status)?;
        let rust = execution::execute(&rust_image, svd, rust_symbol, rust_scenario(case.status))?;
        validate_rust_transaction(&rust, case.status)?;
        let expected = expected_semantic_encoding(case.status);
        let case_matched = rust.return_value == expected;
        matched &= case_matched;
        canonical.push_str(&format!(
            "scenario {} status={:#010x} semantic={expected:#010x} vendor-steps={} rust-steps={}\n",
            case.name, case.status, vendor.steps, rust.steps
        ));
        println!(
            "WDEV-FIQ-MAC-CASE\t{}\t{}\tstatus={:#010x}\tsemantic={:#010x}\tvendor-steps={}\trust-steps={}",
            case.name,
            if case_matched { "MATCH" } else { "MISMATCH" },
            case.status,
            rust.return_value,
            vendor.steps,
            rust.steps,
        );
    }
    println!(
        "WDEV-FIQ-MAC-SUMMARY\twDev_ProcessFiq\t{}\tscenarios={}\tscope=mac-interrupt-domain",
        if matched { "MATCH" } else { "MISMATCH" },
        CASES.len(),
    );
    Ok(DriverAdapterQualification { matched, canonical })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(address: u32, value: u32) -> execution::ExecutionEvent {
        execution::ExecutionEvent::Read {
            width: 32,
            address,
            register: "TEST".to_owned(),
            value,
        }
    }

    fn write(address: u32, value: u32) -> execution::ExecutionEvent {
        execution::ExecutionEvent::Write {
            width: 32,
            address,
            register: "TEST".to_owned(),
            value,
        }
    }

    fn fence() -> execution::ExecutionEvent {
        execution::ExecutionEvent::Fence {
            fm: 0,
            predecessor: 0x0f,
            successor: 0x0f,
        }
    }

    #[test]
    fn semantic_encoding_covers_spurious_supported_and_unhandled_images() {
        assert_eq!(expected_semantic_encoding(0), 0x8000_0082);
        assert_eq!(expected_semantic_encoding(MAC_INT_RX_SUCCESS), 0x8000_018d);
        assert_eq!(
            expected_semantic_encoding(MAC_INT_RX_SUCCESS | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK),
            0x8000_018d
        );
        assert_eq!(
            expected_semantic_encoding(MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK),
            0x8000_0087
        );
        assert_eq!(expected_semantic_encoding(HANDLED_MAC_MASK), 0x8043_21a5);
        assert_eq!(expected_semantic_encoding(0x800), 0x8000_0087);
        assert_eq!(
            expected_semantic_encoding(HANDLED_MAC_MASK | 0x800),
            0x8043_21a5
        );
    }

    #[test]
    fn vendor_call_contract_preserves_work_order_and_drain_iteration() {
        assert_eq!(expected_calls(0), INFRASTRUCTURE_CALLS);
        let calls = expected_calls(HANDLED_MAC_MASK);
        assert_eq!(
            &calls[INFRASTRUCTURE_CALLS.len() + CLEAR_CALLS.len()
                ..INFRASTRUCTURE_CALLS.len() + CLEAR_CALLS.len() + WORK_CALLS.len()],
            &[
                "lmacProcessRxSucData",
                "lmacPostTxComplete",
                "lmacProcessAllTxTimeout",
                "lmacProcessCollisions",
            ]
        );
        assert!(calls.ends_with(INFRASTRUCTURE_CALLS));
    }

    #[test]
    fn composed_irq_transaction_rejects_address_value_order_and_extra_access_mutations() {
        let status = MAC_INT_RX_SUCCESS;
        let exact = [
            read(MAC_STATUS, status),
            write(MAC_CLEAR, status),
            fence(),
            read(MAC_STATUS, 0),
        ];
        assert!(rust_transaction_matches(&exact, status));

        let wrong_address = [
            read(MAC_STATUS, status),
            write(MAC_CLEAR + 4, status),
            fence(),
            read(MAC_STATUS, 0),
        ];
        assert!(!rust_transaction_matches(&wrong_address, status));

        let wrong_value = [
            read(MAC_STATUS, status),
            write(MAC_CLEAR, status ^ 1),
            fence(),
            read(MAC_STATUS, 0),
        ];
        assert!(!rust_transaction_matches(&wrong_value, status));

        let wrong_order = [
            read(MAC_STATUS, status),
            fence(),
            write(MAC_CLEAR, status),
            read(MAC_STATUS, 0),
        ];
        assert!(!rust_transaction_matches(&wrong_order, status));

        let extra_enable_read = [
            read(MAC_STATUS, status),
            read(MAC_STATUS - 8, u32::MAX),
            write(MAC_CLEAR, status),
            fence(),
            read(MAC_STATUS, 0),
        ];
        assert!(!rust_transaction_matches(&extra_enable_read, status));
    }
}
