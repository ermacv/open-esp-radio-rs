//! Rust conformance for the reviewed AP/STA connection-context field in the
//! ESP32-S31 hardware CCMP key image.
//!
//! This adapter intentionally proves a narrow property. The vendor and closed
//! PAC transactions differ in occupancy checks and zeroization, so matching
//! their complete MMIO traces would be the wrong contract. Instead, immutable
//! vendor instruction shapes pin the role propagation and the production Rust
//! key builders are executed to prove the resulting two-bit field.

use std::{collections::BTreeMap, path::Path};

use open_radio_vendor_semantics::{
    DriverAdapterCase, DriverAdapterVerification, EffectDisposition, EffectPolicy, EffectSelector,
};

use crate::{MmioMap, Result, artifact, execution};

use super::{code_closure_sha256, inventory_symbol_sha256};

const ADAPTER_ID: &str = "esp32s31-wifi-key-role-v1";
const VENDOR_SYMBOL: &str = "wDev_Insert_KeyEntry";
const VENDOR_MEMBER: &str = "wdev.o";
const LEAF_SYMBOL: &str = "hal_crypto_set_key_entry";
const LEAF_MEMBER: &str = "hal_crypto.o";

const KEY_VALID_BITMAP: u32 = 0x2010_4814;
const CRYPTO_POLICY_CONTROL: u32 = 0x2010_4810;
const STA_INTERFACE_CONTROL: u32 = 0x2010_4800;
const AP_INTERFACE_CONTROL: u32 = 0x2010_4804;
const KEY_TABLE_BASE: u32 = 0x2010_5800;
const KEY_ENTRY_BYTES: u32 = 40;
const KEY_CONTROL_WORD: u32 = 1;
const STA_KEY_INDEX: u32 = 4;
const AP_KEY_INDEX: u32 = 8;
const CONNECTION_CONTEXT_SHIFT: u32 = 24;
const CONNECTION_CONTEXT_MASK: u32 = 0x3;
const VENDOR_KEY: u32 = 0x3ffd_3000;
const VENDOR_METADATA: u32 = 0x3ffd_3040;

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    interface: u32,
    key_index: u32,
    expected_context: u32,
}

const CASES: [Case; 2] = [
    Case {
        name: "sta-context-zero",
        interface: 0,
        key_index: STA_KEY_INDEX,
        expected_context: 0,
    },
    Case {
        name: "ap-context-one",
        interface: 1,
        key_index: AP_KEY_INDEX,
        expected_context: 1,
    },
];

fn required_policy() -> BTreeMap<EffectSelector, EffectDisposition> {
    [(
        EffectSelector::StateWrite {
            width: 8,
            field: "wifi-key-entry.connection-context".to_owned(),
        },
        EffectDisposition::Required,
    )]
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
            "{VENDOR_SYMBOL} key-role policy differs from the closed projection:\nexpected {expected:#?}\nactual {actual:#?}"
        )
        .into());
    }
    Ok(())
}

fn unique_symbol<'a>(
    symbols: &'a [artifact::ArtifactSymbolDefinition],
    member: Option<&str>,
    name: &str,
) -> Result<&'a artifact::ArtifactSymbolDefinition> {
    let matches = symbols
        .iter()
        .filter(|symbol| {
            symbol.name == name
                && member.is_none_or(|member| symbol.member.as_deref() == Some(member))
        })
        .collect::<Vec<_>>();
    let [symbol] = matches.as_slice() else {
        return Err(format!(
            "expected one {}{name}, found {}",
            member.map_or_else(String::new, |member| format!("{member}::")),
            matches.len()
        )
        .into());
    };
    Ok(symbol)
}

fn require_bytes(
    symbol: &artifact::ArtifactSymbolDefinition,
    offset: usize,
    bytes: &[u8],
    meaning: &str,
) -> Result<()> {
    if symbol.bytes.get(offset..offset + bytes.len()) != Some(bytes) {
        return Err(format!(
            "{} {meaning} changed at relative offset {offset:#x}",
            symbol.name
        )
        .into());
    }
    Ok(())
}

fn require_relocation(
    symbol: &artifact::ArtifactSymbolDefinition,
    address: u32,
    target: &str,
) -> Result<()> {
    let relocation = symbol.relocation(address, artifact::RelocationKind::Call);
    if relocation.map(|relocation| relocation.symbol.as_str()) != Some(target) {
        return Err(format!(
            "{} call relocation at {address:#x} no longer targets {target}",
            symbol.name
        )
        .into());
    }
    Ok(())
}

fn validate_vendor_projection(vendor_inventory: &Path, vendor_artifact: &Path) -> Result<String> {
    let wdev_symbols = artifact::load_code_symbols(
        vendor_inventory,
        VENDOR_SYMBOL,
        artifact::CodeSymbolSelection::Exported,
    )?;
    let wdev = unique_symbol(&wdev_symbols, Some(VENDOR_MEMBER), VENDOR_SYMBOL)?;
    if wdev.address != 0 || wdev.bytes.len() != 0x8e {
        return Err(format!(
            "{VENDOR_MEMBER}::{VENDOR_SYMBOL} boundary changed: address={:#x}, size={:#x}",
            wdev.address,
            wdev.bytes.len()
        )
        .into());
    }
    // `a1` is copied into byte zero of the four-byte metadata record passed
    // as `a3` to the leaf.
    require_bytes(
        wdev,
        0x08,
        &[0x23, 0x02, 0xb1, 0x00],
        "role-to-metadata store",
    )?;
    require_relocation(wdev, 0x3c, LEAF_SYMBOL)?;

    let leaf_symbols = artifact::load_code_symbols(
        vendor_inventory,
        LEAF_SYMBOL,
        artifact::CodeSymbolSelection::Exported,
    )?;
    let leaf = unique_symbol(&leaf_symbols, Some(LEAF_MEMBER), LEAF_SYMBOL)?;
    if leaf.address != 0 || leaf.bytes.len() != 0x1c2 {
        return Err(format!(
            "{LEAF_MEMBER}::{LEAF_SYMBOL} boundary changed: address={:#x}, size={:#x}",
            leaf.address,
            leaf.bytes.len()
        )
        .into());
    }
    require_bytes(leaf, 0xec, &[0x03, 0x47, 0x0a, 0x00], "metadata role load")?;
    require_bytes(leaf, 0xf6, &[0x0d, 0x8b], "two-bit role mask")?;
    require_bytes(
        leaf,
        0xf8,
        &[0x22, 0x07],
        "role shift into control halfword",
    )?;

    let ap_symbols = artifact::load_code_symbols(
        vendor_artifact,
        "esp_wifi_set_ap_key_internal",
        artifact::CodeSymbolSelection::Exported,
    )?;
    let ap = unique_symbol(&ap_symbols, None, "esp_wifi_set_ap_key_internal")?;
    require_bytes(ap, 0xd4, &[0x05, 0x45], "AP context constant one")?;

    let sta_symbols = artifact::load_code_symbols(
        vendor_artifact,
        "ppInstallKey",
        artifact::CodeSymbolSelection::Exported,
    )?;
    let sta = unique_symbol(&sta_symbols, None, "ppInstallKey")?;
    require_bytes(sta, 0xaa, &[0x01, 0x45], "STA context constant zero")?;

    Ok("vendor-ap-context 1\nvendor-sta-context 0\nvendor-role-projection a1 -> metadata[0] & 0x3 -> control-halfword[9:8] / key-word[25:24]\n".to_owned())
}

fn production_scenario(case: Case) -> execution::Scenario {
    let mut scenario = execution::Scenario {
        arguments: vec![0, case.interface],
        max_steps: 2_000_000,
        ..execution::Scenario::default()
    };
    scenario
        .mmio_reads
        .insert(KEY_VALID_BITMAP, [0, 1 << case.key_index].into());
    scenario.mmio_initial.insert(CRYPTO_POLICY_CONTROL, 0);
    scenario.mmio_initial.insert(
        if case.interface == 0 {
            STA_INTERFACE_CONTROL
        } else {
            AP_INTERFACE_CONTROL
        },
        0,
    );
    scenario
}

fn vendor_scenario(case: Case) -> execution::Scenario {
    let mut scenario = execution::Scenario {
        arguments: vec![case.key_index, VENDOR_KEY, 16, VENDOR_METADATA],
        mmio_initial: BTreeMap::from([(KEY_VALID_BITMAP, 0)]),
        max_steps: 20_000,
        ..execution::Scenario::default()
    };
    for offset in 0..16_u32 {
        scenario.memory_initial.insert(VENDOR_KEY + offset, 0);
    }
    // The leaf consumes a nine-byte metadata image. Byte zero is the role
    // propagated by `wDev_Insert_KeyEntry`; the remaining zero values select
    // a deterministic non-special cipher/key shape without affecting the
    // reviewed connection-context bits.
    for offset in 0..9_u32 {
        scenario.memory_initial.insert(VENDOR_METADATA + offset, 0);
    }
    scenario
        .memory_initial
        .insert(VENDOR_METADATA, case.interface as u8);
    scenario
}

fn key_control_address(case: Case) -> u32 {
    KEY_TABLE_BASE + case.key_index * KEY_ENTRY_BYTES + KEY_CONTROL_WORD * 4
}

fn final_key_control(result: &execution::ExecutionResult, case: Case) -> Option<u32> {
    let address = key_control_address(case);
    result.events.iter().rev().find_map(|event| match event {
        execution::ExecutionEvent::Write {
            width: 32,
            address: actual,
            value,
            ..
        } if *actual == address => Some(*value),
        _ => None,
    })
}

pub fn verify_esp32s31_wifi_key_role(
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
        .ok_or("Wi-Fi key-role verification requires the caller-owned raw libpp inventory")?;
    let vendor_projection = validate_vendor_projection(vendor_inventory, vendor_artifact)?;

    let mut vendor_image = execution::ExecutableImage::load(vendor_artifact)?;
    if let Some(companion) = vendor_companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = execution::ExecutableImage::load(rust_artifact)?;
    if let Some(companion) = rust_companion {
        rust_image.add_companion(companion)?;
    }

    let mut canonical = format!(
        "driver-adapter {ADAPTER_ID}\nvendor-inventory-symbol-sha256 {}\nvendor-leaf-inventory-symbol-sha256 {}\nvendor-root-code-closure-sha256 {}\nvendor-leaf-code-closure-sha256 {}\nrust-code-closure-sha256 {}\n",
        inventory_symbol_sha256(vendor_inventory, Some(VENDOR_MEMBER), VENDOR_SYMBOL)?,
        inventory_symbol_sha256(vendor_inventory, Some(LEAF_MEMBER), LEAF_SYMBOL)?,
        code_closure_sha256(&vendor_image, "esp_wifi_set_ap_key_internal")?,
        code_closure_sha256(&vendor_image, LEAF_SYMBOL)?,
        code_closure_sha256(&rust_image, rust_symbol)?,
    );
    canonical.push_str(&vendor_projection);
    canonical.push_str("claim rust-conformance-not-whole-function-equivalence\n");

    let mut matched = true;
    let mut reports = Vec::with_capacity(CASES.len());
    let mut controls = Vec::with_capacity(CASES.len());
    for case in CASES {
        let vendor_result = super::execute_case(
            &vendor_image,
            svd,
            LEAF_SYMBOL,
            vendor_scenario(case),
            format!("vendor-{}", case.name),
            "concrete vendor key-role leaf execution",
        )?;
        let vendor_control = final_key_control(&vendor_result, case);
        let vendor_context =
            vendor_control.map(|word| (word >> CONNECTION_CONTEXT_SHIFT) & CONNECTION_CONTEXT_MASK);
        let vendor_matched = vendor_context == Some(case.expected_context);
        matched &= vendor_matched;
        canonical.push_str(&format!(
            "vendor-scenario {} key-index={} control={:#010x} context={}\n",
            case.name,
            case.key_index,
            vendor_control.unwrap_or(u32::MAX),
            vendor_context.unwrap_or(u32::MAX),
        ));
        reports.push(DriverAdapterCase {
            name: format!("vendor-{}", case.name),
            matched: vendor_matched,
            reason: (!vendor_matched).then(|| {
                format!(
                    "vendor leaf expected context {}, observed {vendor_context:?}",
                    case.expected_context
                )
            }),
        });

        let result = super::execute_case(
            &rust_image,
            svd,
            rust_symbol,
            production_scenario(case),
            format!("production-{}", case.name),
            "production Rust key-role execution",
        )?;
        let control = final_key_control(&result, case);
        let context =
            control.map(|word| (word >> CONNECTION_CONTEXT_SHIFT) & CONNECTION_CONTEXT_MASK);
        let case_matched = result.return_value == 1 && context == Some(case.expected_context);
        matched &= case_matched;
        controls.push(control);
        canonical.push_str(&format!(
            "scenario {} key-index={} control={:#010x} context={} return={}\n",
            case.name,
            case.key_index,
            control.unwrap_or(u32::MAX),
            context.unwrap_or(u32::MAX),
            result.return_value,
        ));
        reports.push(DriverAdapterCase {
            name: format!("production-{}", case.name),
            matched: case_matched,
            reason: (!case_matched).then(|| {
                format!(
                    "expected context {}, observed {context:?}, return {}",
                    case.expected_context, result.return_value
                )
            }),
        });
    }

    let differential = matches!(controls.as_slice(), [Some(sta), Some(ap)] if sta ^ ap == 1 << CONNECTION_CONTEXT_SHIFT);
    matched &= differential;
    canonical.push_str(&format!(
        "cross-role-only-difference-bit-24 {differential}\n"
    ));
    reports.push(DriverAdapterCase {
        name: "ap-sta-context-differential".to_owned(),
        matched: differential,
        reason: (!differential).then(|| format!("control words were {controls:?}")),
    });

    Ok(DriverAdapterVerification::from_trust(
        crate::driver_adapter_trust(ADAPTER_ID).expect("registered adapter has a trust boundary"),
        matched,
        canonical,
    )
    .with_cases(reports))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_control_addresses_are_distinct_and_word_aligned() {
        assert_eq!(key_control_address(CASES[0]), 0x2010_58a4);
        assert_eq!(key_control_address(CASES[1]), 0x2010_5944);
    }
}
