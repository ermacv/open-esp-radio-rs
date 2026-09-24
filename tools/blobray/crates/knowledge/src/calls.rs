//! One reviewed integer/pointer signature profile for functions and slots.
use super::*;
pub(super) fn width(value: &AbiValueType) -> Option<u8> {
    match value {
        AbiValueType::Integer { bits, .. } if matches!(bits, 8 | 16 | 32 | 64) => Some(bits / 8),
        AbiValueType::Pointer { .. } => Some(4),
        _ => None,
    }
}
pub(super) fn validate(signature: &CallSignature) -> Result<()> {
    if signature.arguments.len() > 32
        || signature.variadic
        || (!matches!(signature.result, AbiValueType::Void) && width(&signature.result).is_none())
        || signature
            .arguments
            .iter()
            .any(|a| width(&a.value_type).is_none())
    {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported reviewed signature; expected bounded nonvariadic integer/pointer ABI",
        ));
    }
    Ok(())
}
