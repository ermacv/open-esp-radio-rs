//! Match a reviewed layout against exact bit provenance from structural analysis.

use super::{BitSource, ReviewedCompressedPointerEncoding, SymbolicValue};

fn field_mask(encoding: ReviewedCompressedPointerEncoding) -> Option<u32> {
    match encoding.field_bits() {
        0 => None,
        32 => Some(u32::MAX),
        bits if bits < 32 => Some((1_u32 << bits) - 1),
        _ => None,
    }
}

fn shifted_field_mask(encoding: ReviewedCompressedPointerEncoding) -> Option<u32> {
    let end = encoding
        .field_bits()
        .checked_add(encoding.address_shift())?;
    if end > 32 {
        return None;
    }
    field_mask(encoding)?.checked_shl(u32::from(encoding.address_shift()))
}

pub(super) fn recognizes(
    encoding: ReviewedCompressedPointerEncoding,
    value: &SymbolicValue,
) -> bool {
    if encoding.id().is_empty() {
        return false;
    }
    let Some(shifted_field_mask) = shifted_field_mask(encoding) else {
        return false;
    };
    if encoding.address_base() & shifted_field_mask != 0 {
        return false;
    }

    let bits = value.bits();
    let mut source_token = None;
    for (destination, source) in bits.iter().enumerate() {
        let source_bit = destination.checked_sub(usize::from(encoding.address_shift()));
        if source_bit.is_some_and(|bit| bit < usize::from(encoding.field_bits())) {
            let BitSource::Memory {
                read_token,
                bit,
                inverted: false,
            } = source
            else {
                return false;
            };
            if usize::from(*bit) != source_bit.unwrap() {
                return false;
            }
            match source_token {
                Some(existing) if existing != *read_token => return false,
                Some(_) => {}
                None => source_token = Some(*read_token),
            }
        } else if *source
            != BitSource::Constant(encoding.address_base() & (1_u32 << destination) != 0)
        {
            return false;
        }
    }
    source_token.is_some()
}
