//! Explicit runtime instances of selected reviewed interface contracts.
use crate::*;
pub const MAX_RUNTIME_TABLES: usize = 128;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceReview {
    pub knowledge: KnowledgeRevisionId,
    pub assertion: AssertionId,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RuntimeSlotTarget {
    Service {
        address: u32,
    },
    Null,
    /// Exact executable address in the selected captured mappings.
    Code {
        address: u32,
    },
    /// Exact address of an explicitly selected live call model.
    Model {
        address: u32,
    },
}
impl RuntimeSlotTarget {
    pub fn address(self) -> u32 {
        match self {
            Self::Null => 0,
            Self::Code { address } | Self::Model { address } | Self::Service { address } => address,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSlot {
    pub offset: u32,
    pub target: RuntimeSlotTarget,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTable {
    pub id: String,
    pub review: InterfaceReview,
    pub lifetime: RegionLifetime,
    /// Own the complete table range. Explicit slots overwrite its seed bytes.
    pub seed: MemorySeed,
    pub slots: Vec<RuntimeSlot>,
    /// Existing writable normal-memory cells initialized with the table base.
    pub pointer_cells: Vec<u32>,
}
impl RuntimeTable {
    pub fn validate(&self) -> Result<()> {
        self.seed.validate()?;
        let bad = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid runtime table declaration",
            )
        };
        if self.id.trim().is_empty()
            || self.id.len() > 128
            || !self.id.is_ascii()
            || !self.seed.address.is_multiple_of(4)
            || !self.seed.length.is_multiple_of(4)
            || self.slots.is_empty()
            || self.slots.len() > 64
            || self.pointer_cells.len() > 128
        {
            return Err(bad());
        }
        for (i, s) in self.slots.iter().enumerate() {
            if !s.offset.is_multiple_of(4)
                || s.offset.checked_add(4).is_none_or(|n| n > self.seed.length)
                || self.slots[..i].iter().any(|p| p.offset == s.offset)
            {
                return Err(bad());
            }
            if !matches!(s.target, RuntimeSlotTarget::Null)
                && (s.target.address() == 0
                    || s.target.address() & 1 != 0
                    || s.target.address() >= u32::MAX - 1)
            {
                return Err(bad());
            }
        }
        for (i, p) in self.pointer_cells.iter().enumerate() {
            if !p.is_multiple_of(4) || *p >= u32::MAX - 4 || self.pointer_cells[..i].contains(p) {
                return Err(bad());
            }
        }
        Ok(())
    }
}
/// Captured physical root resolved before session allocation; paths remain explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRoot {
    Address { address: u32 },
    EntryWord { entry: u32, word: u8 },
}
impl RuntimeTable {
    pub fn payload_bytes(&self) -> u64 {
        (self.id.len()
            + 128
            + self.seed.bytes.len()
            + self.slots.len() * std::mem::size_of::<RuntimeSlot>()
            + self.pointer_cells.len() * 4) as u64
    }
    /// Identity binds the exact reviewed snapshot and explicit runtime placement.
    pub fn identity(&self, c: &mut dyn RunControl) -> Result<ArtifactId> {
        use sha2::{Digest, Sha256};
        self.validate()?;
        let mut h = Sha256::new();
        h.update(b"blobray-runtime-table-v1\0");
        for s in [
            self.id.as_str(),
            self.review.knowledge.as_str(),
            self.review.assertion.as_str(),
        ] {
            c.bytes(s.len())?;
            h.update((s.len() as u32).to_le_bytes());
            h.update(s.as_bytes());
        }
        h.update([match self.lifetime {
            RegionLifetime::Phase => 0,
            RegionLifetime::Session => 1,
        }]);
        for n in [
            self.seed.address,
            self.seed.length,
            u32::from(self.seed.fill.is_some()),
            u32::from(self.seed.fill.unwrap_or(0)),
            self.seed.bytes.len() as u32,
        ] {
            h.update(n.to_le_bytes());
        }
        for chunk in self.seed.bytes.chunks(WORK_BLOCK) {
            c.checkpoint(1)?;
            h.update(chunk);
        }
        h.update((self.slots.len() as u32).to_le_bytes());
        for s in &self.slots {
            c.checkpoint(1)?;
            h.update(s.offset.to_le_bytes());
            h.update([match s.target {
                RuntimeSlotTarget::Null => 0,
                RuntimeSlotTarget::Code { .. } => 1,
                RuntimeSlotTarget::Model { .. } => 2,
                RuntimeSlotTarget::Service { .. } => 3,
            }]);
            h.update(s.target.address().to_le_bytes());
        }
        h.update((self.pointer_cells.len() as u32).to_le_bytes());
        for a in &self.pointer_cells {
            c.checkpoint(1)?;
            h.update(a.to_le_bytes());
        }
        Ok(ArtifactId(format!("{:x}", h.finalize())))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RuntimeTableIssue {
    UnknownRoot,
    RootAddressOverflow,
    RootMismatch { actual: u32, expected: u32 },
    IndexPrecondition { word: u8 },
    UnknownPointer { address: u32 },
    GuardUnknown { offset: u32 },
    GuardMismatch { offset: u32, actual: u32 },
    NullTarget,
    UnassociatedTarget { target: u32 },
    AmbiguousTarget { target: u32, candidates: u32 },
    UnavailableTarget { target: u32 },
    UnreviewedModel { target: u32 },
    ModelAbi { target: u32 },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTableObservation {
    pub instance: u16,
    pub id: String,
    pub review: InterfaceReview,
    pub definition: ArtifactId,
    pub lifetime: RegionLifetime,
    pub base: u32,
    pub length: u32,
    pub initialized: u32,
    pub expected_slots: u32,
    pub expected_pointers: u32,
    pub conditions_checked: bool,
    pub pointer_installs: u32,
    pub writes: u64,
    pub calls: u64,
    pub closed: bool,
    pub issue: Option<RuntimeTableIssue>,
    pub status: ModelStatus,
}
impl RuntimeTableObservation {
    pub fn expected_status(&self) -> ModelStatus {
        if self.issue.is_some()
            || self.initialized != self.expected_slots
            || self.pointer_installs != self.expected_pointers
            || !self.conditions_checked
        {
            ModelStatus::Incomplete
        } else if self.closed {
            ModelStatus::Complete
        } else {
            ModelStatus::Open
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RuntimeTableEvent {
    ConditionsChecked,
    Initialized {
        offset: u32,
        target: u32,
    },
    PointerInstalled {
        address: u32,
        base: u32,
    },
    Written {
        site: Option<u32>,
        offset: u32,
        width: u8,
        value: u32,
    },
    /// Unique current slot-value association, not register/load provenance.
    IndirectTarget {
        site: u32,
        target: u32,
        offset: u32,
    },
}
