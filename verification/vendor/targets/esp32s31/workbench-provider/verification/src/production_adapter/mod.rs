//! Concrete vendor replays bound to compiled production Rust entry points.

use std::path::Path;

use sha2::{Digest, Sha256};

mod hal_mac_txq_enable;
mod iq_estimator;
mod sta_join_state;
mod wdev_append_rx_blocks;
mod wdev_process_fiq;
mod wifi_key_role;

pub use hal_mac_txq_enable::*;
pub use iq_estimator::*;
pub use sta_join_state::*;
pub use wdev_append_rx_blocks::*;
pub use wdev_process_fiq::*;
pub use wifi_key_role::*;

use crate::{DriverAdapterVerification, Result, entry_contract, execution, seed_ram_word};

fn execute_case(
    image: &execution::ExecutableImage,
    svd: &crate::MmioMap,
    symbol: &str,
    scenario: execution::Scenario,
    case: impl Into<String>,
    phase: impl Into<String>,
) -> Result<execution::ExecutionResult> {
    let case = case.into();
    let phase = phase.into();
    execution::execute(image, svd, symbol, scenario).map_err(|source| {
        crate::Error::VerificationCase {
            case,
            phase,
            source,
        }
    })
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn inventory_symbol_sha256(path: &Path, member: Option<&str>, symbol_name: &str) -> Result<String> {
    let symbols = crate::artifact::load_code_symbols(
        path,
        symbol_name,
        crate::artifact::CodeSymbolSelection::Exported,
    )?;
    let matches = symbols
        .iter()
        .filter(|symbol| {
            symbol.name == symbol_name
                && member.is_none_or(|member| symbol.member.as_deref() == Some(member))
        })
        .collect::<Vec<_>>();
    let [symbol] = matches.as_slice() else {
        return Err(format!(
            "expected exactly one inventory definition for {}{}, found {}",
            member.map_or_else(String::new, |member| format!("{member}::")),
            symbol_name,
            matches.len()
        )
        .into());
    };

    inventory_symbol_definition_sha256(symbol)
}

fn inventory_symbol_definition_sha256(
    symbol: &crate::artifact::ArtifactSymbolDefinition,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"riscv32-inventory-symbol-v1");
    hash_field(
        &mut hasher,
        symbol.member.as_deref().unwrap_or("").as_bytes(),
    );
    hash_field(&mut hasher, symbol.name.as_bytes());
    hash_field(&mut hasher, &[u8::from(symbol.addresses_resolved)]);
    hash_field(&mut hasher, &symbol.bytes);
    hasher.update((symbol.relocations.len() as u64).to_le_bytes());
    let symbol_start = u32::try_from(symbol.address).map_err(|_| {
        format!(
            "inventory symbol {} exceeds RV32 address space",
            symbol.name
        )
    })?;
    for relocation in &symbol.relocations {
        let relative = relocation
            .address
            .checked_sub(symbol_start)
            .ok_or_else(|| {
                format!(
                    "relocation at {:#x} precedes inventory symbol {}",
                    relocation.address, symbol.name
                )
            })?;
        hasher.update(relative.to_le_bytes());
        let kind = match relocation.kind {
            crate::artifact::RelocationKind::GotHi20 => b"got-hi20".as_slice(),
            crate::artifact::RelocationKind::Hi20 => b"hi20".as_slice(),
            crate::artifact::RelocationKind::Lo12I => b"lo12-i".as_slice(),
            crate::artifact::RelocationKind::Lo12S => b"lo12-s".as_slice(),
            crate::artifact::RelocationKind::PcRelHi20 => b"pcrel-hi20".as_slice(),
            crate::artifact::RelocationKind::PcRelLo12I => b"pcrel-lo12-i".as_slice(),
            crate::artifact::RelocationKind::PcRelLo12S => b"pcrel-lo12-s".as_slice(),
            crate::artifact::RelocationKind::GotPcRelLo12I => b"got-pcrel-lo12-i".as_slice(),
            crate::artifact::RelocationKind::Call => b"call".as_slice(),
            crate::artifact::RelocationKind::CallPlt => b"call-plt".as_slice(),
        };
        hash_field(&mut hasher, kind);
        hash_field(&mut hasher, relocation.symbol.as_bytes());
        hasher.update(relocation.addend.to_le_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn code_closure_sha256(image: &execution::ExecutableImage, symbol: &str) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(image.code_closure_identity(symbol)?.as_bytes())
    ))
}
