//! Shared reviewed ABI values for direct functions and interface slots.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AbiValueType {
    Void,
    Integer { bits: u8, signed: bool },
    Pointer { nullable: bool },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallArgument {
    pub role: SubjectId,
    pub value_type: AbiValueType,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallSignature {
    pub arguments: Vec<CallArgument>,
    pub result: AbiValueType,
    pub variadic: bool,
}

impl CallSignature {
    pub fn allocated_bytes(&self) -> u64 {
        (self.arguments.capacity() * std::mem::size_of::<CallArgument>()) as u64
            + self
                .arguments
                .iter()
                .map(|a| a.role.allocated_bytes())
                .sum::<u64>()
    }
}

impl CallSignature {
    /// RV32 nonvariadic scalar ABI word containing the low word of this argument.
    /// Words 0..7 are a0..a7; word 8 is entry SP + 0. This performs no execution.
    pub fn argument_word(&self, argument: u8) -> Option<u8> {
        if self.variadic || self.arguments.len() > 32 || argument as usize >= self.arguments.len() {
            return None;
        }
        let mut word = 0u8;
        for (i, value) in self.arguments.iter().enumerate() {
            let count = match value.value_type {
                AbiValueType::Integer {
                    bits: 8 | 16 | 32, ..
                }
                | AbiValueType::Pointer { .. } => 1,
                AbiValueType::Integer { bits: 64, .. } => 2,
                _ => return None,
            };
            // A 64-bit scalar may split a7/stack; only a wholly stacked scalar aligns to 8 bytes.
            if word >= 8 && count == 2 {
                word = word.checked_add(word % 2)?;
            }
            if i == argument as usize {
                return Some(word);
            }
            word = word.checked_add(count)?;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn signature(bits: &[u8]) -> CallSignature {
        CallSignature {
            arguments: bits
                .iter()
                .map(|bits| CallArgument {
                    role: "test.argument".to_owned().try_into().unwrap(),
                    value_type: AbiValueType::Integer {
                        bits: *bits,
                        signed: false,
                    },
                })
                .collect(),
            result: AbiValueType::Void,
            variadic: false,
        }
    }
    #[test]
    fn named_rv32_arguments_preserve_register_split_and_stack_alignment() {
        for (bits, expected) in [
            (vec![32, 64, 32], vec![0, 1, 3]),
            (
                vec![32, 32, 32, 32, 32, 32, 32, 64, 32, 64],
                vec![0, 1, 2, 3, 4, 5, 6, 7, 9, 10],
            ),
            (
                vec![32, 32, 32, 32, 32, 32, 32, 32, 32, 64],
                vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 10],
            ),
        ] {
            let s = signature(&bits);
            assert_eq!(
                (0..bits.len())
                    .map(|i| s.argument_word(i as u8).unwrap())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(s.argument_word(bits.len() as u8), None);
        }
        assert_eq!(signature(&[64; 32]).argument_word(31), Some(62));
        assert_eq!(signature(&[64; 33]).argument_word(0), None);
        assert_eq!(signature(&[24]).argument_word(0), None);
        let mut s = signature(&[32]);
        s.variadic = true;
        assert_eq!(s.argument_word(0), None);
    }
}
