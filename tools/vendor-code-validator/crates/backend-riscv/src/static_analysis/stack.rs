//! Byte-precise private stack image and RV32 call argument capture.

use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SymbolicStack {
    bytes: BTreeMap<i32, [BitSource; 8]>,
}

impl SymbolicStack {
    pub fn store(&mut self, offset: i32, width: u8, value: &SymbolicValue) {
        let bits = value.bits();
        for byte in 0..usize::from(width / 8) {
            self.bytes.insert(
                offset.wrapping_add(byte as i32),
                core::array::from_fn(|bit| bits[byte * 8 + bit]),
            );
        }
    }

    pub fn load(&self, offset: i32, width: u8, signed: bool) -> Option<SymbolicValue> {
        let width = usize::from(width);
        let mut bits = [BitSource::Constant(false); 32];
        for destination in 0..width {
            let byte = self
                .bytes
                .get(&offset.wrapping_add((destination / 8) as i32))?;
            bits[destination] = byte[destination % 8];
        }
        if signed {
            let sign = bits[width - 1];
            bits[width..].fill(sign);
        }
        Some(SymbolicValue::from_bits(bits))
    }
}

pub(super) fn structural_call_arguments(
    values: &[SymbolicValue; 32],
    stack: &SymbolicStack,
    private_stack_may_be_modified_by_call: bool,
) -> Box<Rv32CallArguments> {
    Box::new(core::array::from_fn(|index| {
        if index < RV32_REGISTER_ARGUMENT_COUNT {
            return values[10 + index].clone();
        }
        let stack_index = index - RV32_REGISTER_ARGUMENT_COUNT;
        let Some(offset) = values[usize::from(Reg::SP.0)]
            .private_stack_offset()
            .map(|base| base.wrapping_add((stack_index * 4) as i32))
        else {
            return SymbolicValue::Unknown;
        };
        (!private_stack_may_be_modified_by_call)
            .then(|| stack.load(offset, 32, false))
            .flatten()
            .unwrap_or(SymbolicValue::Unknown)
    }))
}
