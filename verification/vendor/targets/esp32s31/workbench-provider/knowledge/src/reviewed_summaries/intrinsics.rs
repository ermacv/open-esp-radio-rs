//! Reviewed chip-specific arithmetic intrinsics.

use super::body_identity::*;
use super::*;

pub(super) const ROM_DIVDI3_SIZE: usize = 926;
pub(super) const ROM_DIVDI3_ADDRESS: u32 = crate::wide_signed_divide_target_address();

pub(super) fn exact_wide_signed_divide(symbol: &artifact::ArtifactSymbolDefinition) -> bool {
    reviewed_identity_matches(
        ReviewedBodyIdentity {
            name: &symbol.name,
            address: symbol.address,
            size: symbol.bytes.len(),
        },
        ReviewedBodyIdentity {
            name: "__divdi3",
            address: u64::from(ROM_DIVDI3_ADDRESS),
            size: ROM_DIVDI3_SIZE,
        },
    )
}

pub(crate) fn wide_signed_divide_intrinsic(
    symbol: &artifact::ArtifactSymbolDefinition,
    arguments: &Rv32CallArguments,
) -> Option<(SymbolicValue, SymbolicValue)> {
    exact_wide_signed_divide(symbol).then(|| {
        SymbolicValue::wide_signed_divide_words(
            arguments[0].clone(),
            arguments[1].clone(),
            arguments[2].clone(),
            arguments[3].clone(),
        )
    })
}
