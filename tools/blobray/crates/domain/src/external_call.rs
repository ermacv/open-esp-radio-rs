//! Explicit ABI boundary assumptions; no implicit return or memory owner.
use crate::*;
use sha2::{Digest, Sha256};
pub const MAX_CALL_MODELS: usize = 128;
pub const MAX_CALL_RESPONSES: usize = 4096;
pub const MAX_CALL_OUTPUTS: usize = 256;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallBoundary {
    Unmapped,
    CapturedCode,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallBinding {
    pub address: u32,
    pub boundary: CallBoundary,
    pub allow_tail: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallDeclaration {
    pub id: String,
    pub applicability: String,
    pub lifetime: RegionLifetime,
    pub binding: CallBinding,
    /// Physical ABI words recorded at each modeled call; no type/layout inference.
    pub argument_words: u16,
    pub responses: Vec<CallResponse>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallOutputScope {
    PrivateStack,
    NormalMemory,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallOutput {
    pub pointer_argument: u16,
    pub byte_offset: u32,
    pub width: u8,
    pub value: u32,
    pub scope: CallOutputScope,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallAllocation {
    pub address: u32,
    pub size_argument: u16,
    pub capacity: u32,
    pub lifetime: RegionLifetime,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CallValue {
    Constant { value: u32 },
    Argument { word: u16 },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallResponse {
    pub return_words: [Option<u32>; 2],
    pub outputs: Vec<CallOutput>,
    pub allocation: Option<CallAllocation>,
    /// Explicit modeled microseconds, never wall time or hardware timing evidence.
    pub delay_micros: Option<CallValue>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CallIssue {
    ExhaustedResponses,
    TailNotAllowed,
    StackAlignment,
    StackArgument {
        word: u16,
    },
    UnknownArgument {
        word: u16,
    },
    OutputAddress {
        word: u16,
    },
    OutputAccess {
        address: u32,
        scope: CallOutputScope,
    },
    AllocationSize {
        requested: u32,
        capacity: u32,
    },
    AllocationOverlap {
        address: u32,
        capacity: u32,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallObservation {
    pub id: String,
    pub definition: ArtifactId,
    pub lifetime: RegionLifetime,
    pub calls: u32,
    pub remaining: u32,
    pub closed: bool,
    pub issue: Option<CallIssue>,
    pub status: ModelStatus,
}
impl CallObservation {
    pub fn expected_status(&self) -> ModelStatus {
        if self.issue.is_some() || (self.closed && self.remaining != 0) {
            ModelStatus::Incomplete
        } else if self.closed {
            ModelStatus::Complete
        } else {
            ModelStatus::Open
        }
    }
}
/// Borrowed machine ABI state at a transfer, before any model response/clobber.
#[derive(Clone, Copy, Debug)]
pub struct CallInput {
    pub site: u32,
    pub target: u32,
    pub tail: bool,
    pub indirect: bool,
    pub stack: Option<u32>,
    pub arguments: [Option<u32>; 8],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallDispatch {
    RuntimeInterface {
        instance: Option<u16>,
        issue: RuntimeTableIssue,
    },
    /// No selected model claims this transfer; execute captured code normally.
    Code,
    Returned {
        words: [Option<u32>; 2],
    },
    Incomplete {
        issue: CallIssue,
    },
}
impl CallDeclaration {
    pub fn validate(&self) -> Result<()> {
        let bad = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid external-call declaration",
            )
        };
        if self.id.trim().is_empty()
            || self.id.len() > 128
            || !self.id.is_ascii()
            || self.applicability.trim().is_empty()
            || self.applicability.len() > 1024
            || self.binding.address & 1 != 0
            || self.binding.address >= u32::MAX - 1
            || usize::from(self.argument_words) > MAX_EXECUTION_ARGUMENT_WORDS
            || self.responses.len() > MAX_CALL_RESPONSES
        {
            return Err(bad());
        }
        for r in &self.responses {
            if r.outputs.len() > MAX_CALL_OUTPUTS {
                return Err(bad());
            }
            for o in &r.outputs {
                if o.pointer_argument >= self.argument_words
                    || !matches!(o.width, 1 | 2 | 4)
                    || (o.width < 4 && o.value >> (o.width * 8) != 0)
                {
                    return Err(bad());
                }
            }
            if let Some(a) = r.allocation
                && (a.size_argument >= self.argument_words
                    || a.capacity == 0
                    || a.address % 16 != 0
                    || u64::from(a.address) + u64::from(a.capacity) >= u64::from(u32::MAX - 1)
                    || r.return_words[0] != Some(a.address))
            {
                return Err(bad());
            }
            if let Some(CallValue::Argument { word }) = r.delay_micros
                && word >= self.argument_words
            {
                return Err(bad());
            }
        }
        Ok(())
    }
    pub fn payload_bytes(&self) -> u64 {
        (self.id.len()
            + self.applicability.len()
            + self.responses.len() * std::mem::size_of::<CallResponse>()
            + self
                .responses
                .iter()
                .map(|r| r.outputs.len() * std::mem::size_of::<CallOutput>())
                .sum::<usize>()) as u64
    }
    /// Stable v1 tagged little-endian encoding of every declared field.
    pub fn identity(&self, c: &mut dyn RunControl) -> Result<ArtifactId> {
        self.validate()?;
        let mut h = Sha256::new();
        h.update(b"blobray-call-v1\0");
        for s in [&self.id, &self.applicability] {
            c.bytes(s.len())?;
            h.update((s.len() as u32).to_le_bytes());
            h.update(s.as_bytes());
        }
        let life = |l| match l {
            RegionLifetime::Phase => 0,
            RegionLifetime::Session => 1,
        };
        let mut put = |n: u32| -> Result<()> {
            c.checkpoint(1)?;
            h.update(n.to_le_bytes());
            Ok(())
        };
        for n in [
            life(self.lifetime),
            self.binding.address,
            match self.binding.boundary {
                CallBoundary::Unmapped => 0,
                CallBoundary::CapturedCode => 1,
            },
            u32::from(self.binding.allow_tail),
            u32::from(self.argument_words),
            self.responses.len() as u32,
        ] {
            put(n)?;
        }
        for r in &self.responses {
            for word in r.return_words {
                put(u32::from(word.is_some()))?;
                put(word.unwrap_or(0))?;
            }
            put(r.outputs.len() as u32)?;
            for o in &r.outputs {
                for n in [
                    u32::from(o.pointer_argument),
                    o.byte_offset,
                    u32::from(o.width),
                    o.value,
                    match o.scope {
                        CallOutputScope::PrivateStack => 0,
                        CallOutputScope::NormalMemory => 1,
                    },
                ] {
                    put(n)?;
                }
            }
            put(u32::from(r.allocation.is_some()))?;
            if let Some(a) = r.allocation {
                for n in [
                    a.address,
                    u32::from(a.size_argument),
                    a.capacity,
                    life(a.lifetime),
                ] {
                    put(n)?;
                }
            }
            match r.delay_micros {
                None => put(0)?,
                Some(CallValue::Constant { value }) => {
                    put(1)?;
                    put(value)?;
                }
                Some(CallValue::Argument { word }) => {
                    put(2)?;
                    put(u32::from(word))?;
                }
            }
        }
        Ok(ArtifactId(format!("{:x}", h.finalize())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_includes_boundary_conditions_responses_and_effect_ownership() {
        let d = CallDeclaration {
            id: "fixture".into(),
            applicability: "fixture assumption".into(),
            lifetime: RegionLifetime::Session,
            binding: CallBinding {
                address: 0x2000,
                boundary: CallBoundary::Unmapped,
                allow_tail: false,
            },
            argument_words: 1,
            responses: vec![CallResponse {
                return_words: [Some(0x4000), None],
                outputs: vec![CallOutput {
                    pointer_argument: 0,
                    byte_offset: 0,
                    width: 4,
                    value: 7,
                    scope: CallOutputScope::NormalMemory,
                }],
                allocation: Some(CallAllocation {
                    address: 0x4000,
                    size_argument: 0,
                    capacity: 16,
                    lifetime: RegionLifetime::Session,
                }),
                delay_micros: Some(CallValue::Constant { value: 5 }),
            }],
        };
        let identity = d.identity(&mut || Ok(())).unwrap();
        for variant in 0..10 {
            let mut changed = d.clone();
            match variant {
                0 => changed.applicability.push('2'),
                1 => changed.lifetime = RegionLifetime::Phase,
                2 => changed.binding.boundary = CallBoundary::CapturedCode,
                3 => changed.binding.address += 4,
                4 => changed.binding.allow_tail = true,
                5 => changed.argument_words = 2,
                6 => changed.responses[0].return_words[1] = Some(0),
                7 => changed.responses[0].outputs[0].scope = CallOutputScope::PrivateStack,
                8 => {
                    changed.responses[0].allocation.as_mut().unwrap().lifetime =
                        RegionLifetime::Phase
                }
                9 => changed.responses[0].delay_micros = Some(CallValue::Argument { word: 0 }),
                _ => unreachable!(),
            }
            assert_ne!(identity, changed.identity(&mut || Ok(())).unwrap());
        }
    }
}
