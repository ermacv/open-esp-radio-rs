//! Explicit reviewed callback bindings to bounded session-owned FIFO mechanisms.
use crate::*;
pub const MAX_FIFO_SERVICES: usize = 128;
pub const MAX_FIFO_BINDINGS: usize = 64;
pub const MAX_FIFO_ITEMS: u32 = 65536;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FifoInput {
    Argument { word: u16, width: u8 },
    PrivateStack { word: u16, width: u8 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FifoOutput {
    /// Pointer to existing private-stack memory; never RAM or MMIO fallback.
    pub word: u16,
    pub width: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FifoOperation {
    Enqueue {
        input: FifoInput,
        success: u32,
        full: u32,
        wake: Option<FifoOutput>,
    },
    Dequeue {
        output: FifoOutput,
        success: u32,
        empty: u32,
    },
    Length,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FifoBinding {
    pub table: String,
    pub slot: u32,
    pub call: CallBinding,
    pub argument_words: u16,
    pub handle_word: u16,
    pub operation: FifoOperation,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FifoService {
    pub id: String,
    pub applicability: String,
    pub lifetime: RegionLifetime,
    pub handle: u32,
    pub item_width: u8,
    pub capacity: u32,
    pub items: Vec<u32>,
    pub bindings: Vec<FifoBinding>,
}
impl FifoService {
    pub fn validate(&self) -> Result<()> {
        let invalid = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid FIFO service declaration",
            )
        };
        if self.id.trim().is_empty()
            || self.id.len() > 128
            || !self.id.is_ascii()
            || self.applicability.trim().is_empty()
            || self.applicability.len() > 1024
            || !matches!(self.item_width, 1 | 2 | 4)
            || self.handle == 0
            || self.capacity == 0
            || self.capacity > MAX_FIFO_ITEMS
            || self.items.len() > self.capacity as usize
            || self.items.iter().any(|v| !self.fits(*v))
            || self.bindings.is_empty()
            || self.bindings.len() > MAX_FIFO_BINDINGS
        {
            return Err(invalid());
        }
        for (index, b) in self.bindings.iter().enumerate() {
            if b.table.trim().is_empty()
                || b.table.len() > 128
                || !b.table.is_ascii()
                || !b.slot.is_multiple_of(4)
                || b.call.address == 0
                || b.call.address & 1 != 0
                || b.call.address >= u32::MAX - 1
                || b.argument_words as usize > MAX_EXECUTION_ARGUMENT_WORDS
                || b.handle_word >= b.argument_words
                || self.bindings[..index].iter().any(|p| {
                    p.call.address == b.call.address || (p.table == b.table && p.slot == b.slot)
                })
            {
                return Err(invalid());
            }
            let output = |o: FifoOutput| o.word < b.argument_words && matches!(o.width, 1 | 2 | 4);
            match b.operation {
                FifoOperation::Enqueue { input, wake, .. } => {
                    let (word, width) = match input {
                        FifoInput::Argument { word, width }
                        | FifoInput::PrivateStack { word, width } => (word, width),
                    };
                    if word >= b.argument_words
                        || width != self.item_width
                        || wake.is_some_and(|o| !output(o))
                    {
                        return Err(invalid());
                    }
                }
                FifoOperation::Dequeue { output: o, .. }
                    if !output(o) || o.width != self.item_width =>
                {
                    return Err(invalid());
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn fits(&self, value: u32) -> bool {
        match self.item_width {
            1 => value <= 0xff,
            2 => value <= 0xffff,
            4 => true,
            _ => false,
        }
    }
    pub fn payload_bytes(&self) -> u64 {
        (self.id.len()
            + self.applicability.len()
            + self.items.len() * 4
            + self.bindings.len() * std::mem::size_of::<FifoBinding>()
            + self.bindings.iter().map(|b| b.table.len()).sum::<usize>()) as u64
    }
    /// Stable identity of the selected mechanism, explicit bindings and initial queue.
    pub fn identity(&self, c: &mut dyn RunControl) -> Result<ArtifactId> {
        use sha2::{Digest, Sha256};
        self.validate()?;
        let mut h = Sha256::new();
        h.update(b"blobray-fifo-service-v1\0");
        fn string(h: &mut Sha256, v: &str) {
            h.update((v.len() as u32).to_le_bytes());
            h.update(v.as_bytes());
        }
        fn output(h: &mut Sha256, o: FifoOutput) {
            h.update(o.word.to_le_bytes());
            h.update([o.width]);
        }
        c.bytes(self.id.len() + self.applicability.len())?;
        string(&mut h, &self.id);
        string(&mut h, &self.applicability);
        h.update([
            match self.lifetime {
                RegionLifetime::Phase => 0,
                RegionLifetime::Session => 1,
            },
            self.item_width,
        ]);
        h.update(self.handle.to_le_bytes());
        h.update(self.capacity.to_le_bytes());
        h.update((self.items.len() as u32).to_le_bytes());
        for v in &self.items {
            c.checkpoint(1)?;
            h.update(v.to_le_bytes());
        }
        h.update((self.bindings.len() as u32).to_le_bytes());
        for b in &self.bindings {
            c.bytes(b.table.len())?;
            string(&mut h, &b.table);
            h.update(b.slot.to_le_bytes());
            h.update(b.call.address.to_le_bytes());
            h.update([
                match b.call.boundary {
                    CallBoundary::Unmapped => 0,
                    CallBoundary::CapturedCode => 1,
                },
                u8::from(b.call.allow_tail),
            ]);
            h.update(b.argument_words.to_le_bytes());
            h.update(b.handle_word.to_le_bytes());
            match b.operation {
                FifoOperation::Length => h.update([0]),
                FifoOperation::Enqueue {
                    input,
                    success,
                    full,
                    wake,
                } => {
                    h.update([1]);
                    let (kind, word, width) = match input {
                        FifoInput::Argument { word, width } => (0, word, width),
                        FifoInput::PrivateStack { word, width } => (1, word, width),
                    };
                    h.update([kind, width]);
                    h.update(word.to_le_bytes());
                    h.update(success.to_le_bytes());
                    h.update(full.to_le_bytes());
                    h.update([u8::from(wake.is_some())]);
                    if let Some(o) = wake {
                        output(&mut h, o);
                    }
                }
                FifoOperation::Dequeue {
                    output: o,
                    success,
                    empty,
                } => {
                    h.update([2]);
                    output(&mut h, o);
                    h.update(success.to_le_bytes());
                    h.update(empty.to_le_bytes());
                }
            }
        }
        Ok(ArtifactId(format!("{:x}", h.finalize())))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FifoIssue {
    Call { issue: CallIssue },
    Handle { expected: u32, actual: u32 },
    ItemWidth { value: u32 },
    InputAccess { address: u32 },
    UnreviewedBinding { target: u32 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FifoTransition {
    Enqueued { value: u32, woke: bool },
    Dequeued { value: u32 },
    Full { value: u32 },
    Empty,
    Length,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FifoObservation {
    pub instance: u16,
    pub id: String,
    pub definition: ArtifactId,
    pub lifetime: RegionLifetime,
    pub operations: u64,
    pub depth: u32,
    pub closed: bool,
    pub issue: Option<FifoIssue>,
    pub status: ModelStatus,
}
impl FifoObservation {
    pub fn expected_status(&self) -> ModelStatus {
        if self.issue.is_some() {
            ModelStatus::Incomplete
        } else if self.closed {
            ModelStatus::Complete
        } else {
            ModelStatus::Open
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn declaration() -> FifoService {
        FifoService {
            id: "queue".into(),
            applicability: "fixture".into(),
            lifetime: RegionLifetime::Session,
            handle: 1,
            item_width: 1,
            capacity: 2,
            items: vec![7],
            bindings: vec![FifoBinding {
                table: "callbacks".into(),
                slot: 4,
                call: CallBinding {
                    address: 0x2000,
                    boundary: CallBoundary::Unmapped,
                    allow_tail: false,
                },
                argument_words: 2,
                handle_word: 0,
                operation: FifoOperation::Enqueue {
                    input: FifoInput::Argument { word: 1, width: 1 },
                    success: 1,
                    full: 0,
                    wake: None,
                },
            }],
        }
    }
    #[test]
    fn widths_bounds_and_binding_identity_are_explicit() {
        let d = declaration();
        d.validate().unwrap();
        let id = d.identity(&mut || Ok(())).unwrap();
        for variant in 0..6 {
            let mut invalid = d.clone();
            match variant {
                0 => invalid.item_width = 0,
                1 => invalid.capacity = 0,
                2 => invalid.items = vec![256],
                3 => invalid.bindings[0].handle_word = 2,
                4 => invalid.bindings[0].call.address = 0,
                _ => invalid.bindings.push(invalid.bindings[0].clone()),
            };
            assert!(invalid.validate().is_err());
        }
        for variant in 0..6 {
            let mut changed = d.clone();
            match variant {
                0 => changed.items[0] = 8,
                1 => changed.lifetime = RegionLifetime::Phase,
                2 => changed.bindings[0].slot = 8,
                3 => changed.bindings[0].call.boundary = CallBoundary::CapturedCode,
                4 => changed.handle = 2,
                _ => changed.bindings[0].operation = FifoOperation::Length,
            };
            assert_ne!(changed.identity(&mut || Ok(())).unwrap(), id);
        }
    }
}
