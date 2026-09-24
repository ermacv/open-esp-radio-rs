//! Bounded validation of model declarations and lifetime evidence, not device execution.
use super::*;
struct Live<'a> {
    declaration: &'a DeviceDeclaration,
    definition: ArtifactId,
    reads: u64,
    writes: u64,
    issue: Option<DeviceIssue>,
    seen: bool,
    closed: bool,
}
pub(super) struct Models<'a> {
    live: Vec<Live<'a>>,
}
impl<'a> Models<'a> {
    pub fn new() -> Self {
        Self { live: Vec::new() }
    }
    /// Outer execution-manifest admission covers these bounded borrowed entries.
    pub fn begin(
        &mut self,
        declarations: &'a [DeviceDeclaration],
        reset: SessionReset,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if reset == SessionReset::Cold && !self.live.is_empty() {
            return Err(integrity("cold reset has unclosed model evidence"));
        }
        for instance in &mut self.live {
            instance.seen = false;
        }
        if blocked {
            return Ok(());
        }
        for declaration in declarations {
            c.checkpoint(self.live.len() as u64 + 1)?;
            if self.live.len() == MAX_DEVICE_MODELS
                || self.live.iter().any(|m| m.declaration.id == declaration.id)
            {
                return Err(integrity(
                    "duplicate or excessive active model declarations",
                ));
            }
            self.live.try_reserve(1).map_err(|_| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "model validation allocation refused",
                )
            })?;
            self.live.push(Live {
                declaration,
                definition: declaration.identity(c)?,
                reads: 0,
                writes: 0,
                issue: None,
                seen: false,
                closed: false,
            });
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        observed: &ModelObservation,
        close_chain: bool,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        c.checkpoint(self.live.len() as u64 + 1)?;
        let instance = self
            .live
            .iter_mut()
            .find(|i| i.declaration.id == observed.id)
            .ok_or_else(|| integrity("model observation has no live declaration"))?;
        if instance.seen
            || observed.definition != instance.definition
            || observed.lifetime != instance.declaration.lifetime
            || observed.closed != (close_chain || observed.lifetime == RegionLifetime::Phase)
            || observed.status != observed.expected_status()
            || observed.reads < instance.reads
            || observed.writes < instance.writes
            || (instance.issue.is_some() && observed.issue != instance.issue)
            || (blocked
                && (observed.reads != instance.reads
                    || observed.writes != instance.writes
                    || observed.issue != instance.issue))
        {
            return Err(integrity("model identity, counters or closure differs"));
        }
        let (reads, writes) = match &instance.declaration.behavior {
            DeviceBehavior::SequenceRead { values, .. } => (Some(values.len()), Some(0)),
            DeviceBehavior::Fifo { reads, writes, .. } => (Some(reads.len()), Some(writes.len())),
            DeviceBehavior::ConstantRead { .. } | DeviceBehavior::ReadClear { .. } => {
                (None, Some(0))
            }
            _ => (None, None),
        };
        for (expected, used, remaining) in [
            (reads, observed.reads, observed.remaining_reads),
            (writes, observed.writes, observed.remaining_writes),
        ] {
            if match expected {
                Some(expected) => used.checked_add(u64::from(remaining)) != Some(expected as u64),
                None => remaining != 0,
            } {
                return Err(integrity("model obligations differ from its declaration"));
            }
        }
        instance.seen = true;
        instance.closed = observed.closed;
        instance.reads = observed.reads;
        instance.writes = observed.writes;
        instance.issue = observed.issue;
        Ok(())
    }
    pub fn finish_side(&mut self) -> Result<()> {
        if self.live.iter().any(|i| !i.seen) {
            return Err(integrity("missing model participation evidence"));
        }
        self.live.retain(|i| !i.closed);
        Ok(())
    }
    pub fn closed(&self) -> bool {
        self.live.is_empty()
    }
}
