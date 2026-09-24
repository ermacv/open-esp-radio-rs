//! Session-owned device instances and an admitted exact-port index.
use crate::*;
struct Instance<'m> {
    declaration: DeviceDeclaration,
    commands: Option<crate::command_bank::CommandState<'m>>,
    definition: ArtifactId,
    _payload: MemoryReservation<'m>,
    cells: AdmittedVec<'m, RegisterCell>,
    values: AdmittedVec<'m, u32>,
    value: u32,
    index: Option<u32>,
    reads: u64,
    writes: u64,
    read_cursor: usize,
    write_cursor: usize,
    issue: Option<DeviceIssue>,
}
#[derive(Clone, Copy)]
struct Port {
    address: u32,
    width: u8,
    model: usize,
    slot: usize,
}
pub(crate) struct Devices<'m> {
    memory: &'m WorkingMemory,
    instances: AdmittedVec<'m, Option<Instance<'m>>>,
    ports: AdmittedVec<'m, Port>,
}
impl<'m> Devices<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            instances: AdmittedVec::new(memory),
            ports: AdmittedVec::new(memory),
        }
    }
    pub fn overlaps(&self, address: u32, length: u64, c: &mut dyn RunControl) -> Result<bool> {
        c.checkpoint(self.ports.len().max(1).ilog2() as u64 + 1)?;
        let end = u64::from(address) + length;
        let i = self
            .ports
            .partition_point(|p| u64::from(p.address) + u64::from(p.width) <= u64::from(address));
        Ok(self
            .ports
            .get(i)
            .is_some_and(|p| u64::from(p.address) < end))
    }
    pub fn install(
        &mut self,
        declarations: &[DeviceDeclaration],
        c: &mut dyn RunControl,
        check: &mut dyn FnMut(u32, u8, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        for declaration in declarations {
            c.checkpoint(self.instances.len() as u64 + 1)?;
            if self
                .instances
                .iter()
                .flatten()
                .any(|i| i.declaration.id == declaration.id)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "device id is already live; omit it on warm continuation",
                ));
            }
            let instance = Instance::new(declaration, self.memory, c)?;
            if let Some(slot) = self.instances.iter().position(Option::is_none) {
                self.instances[slot] = Some(instance);
            } else {
                if self.instances.len() == MAX_DEVICE_MODELS {
                    return Err(Error::new(
                        ErrorCode::ResourceLimited,
                        "active device capacity exhausted",
                    ));
                }
                self.instances.push(Some(instance), c.position())?;
            }
        }
        self.rebuild(c)?;
        for port in &*self.ports {
            check(port.address, port.width, c)?;
        }
        Ok(())
    }
    fn rebuild(&mut self, c: &mut dyn RunControl) -> Result<()> {
        while self.ports.pop().is_some() {}
        for (model, instance) in self.instances.iter().enumerate() {
            let Some(instance) = instance else {
                continue;
            };
            let mut push = |address, width, slot| -> Result<()> {
                c.checkpoint(1)?;
                if self.ports.len() == MAX_DEVICE_PORTS {
                    return Err(Error::new(
                        ErrorCode::ResourceLimited,
                        "device port capacity exhausted",
                    ));
                }
                self.ports.push(
                    Port {
                        address,
                        width,
                        model,
                        slot,
                    },
                    c.position(),
                )
            };
            match &instance.declaration.behavior {
                DeviceBehavior::CommandBank(bank) => {
                    for (slot, port) in bank.ports.iter().enumerate() {
                        push(port.address, 4, slot)?;
                    }
                }
                DeviceBehavior::RegisterBank { cells } => {
                    for (slot, cell) in cells.iter().enumerate() {
                        push(cell.address, cell.width, slot)?;
                    }
                }
                DeviceBehavior::IndexedBank {
                    index_address,
                    data_address,
                    width,
                    ..
                } => {
                    push(*index_address, *width, 0)?;
                    push(*data_address, *width, 1)?;
                }
                DeviceBehavior::ConstantRead { address, width, .. }
                | DeviceBehavior::SequenceRead { address, width, .. }
                | DeviceBehavior::W1c { address, width, .. }
                | DeviceBehavior::ReadClear { address, width, .. }
                | DeviceBehavior::SelfClearing { address, width, .. }
                | DeviceBehavior::Fifo { address, width, .. } => push(*address, *width, 0)?,
            }
        }
        c.checkpoint(self.ports.len() as u64 * (self.ports.len().max(1).ilog2() as u64 + 1))?;
        self.ports.sort_unstable_by_key(|p| p.address);
        if self
            .ports
            .windows(2)
            .any(|p| u64::from(p[0].address) + u64::from(p[0].width) > u64::from(p[1].address))
        {
            return Err(Error::new(ErrorCode::Conflict, "device ports overlap"));
        }
        Ok(())
    }
    fn port(&self, address: u32, width: u8, c: &mut dyn RunControl) -> Result<Option<Port>> {
        c.checkpoint(self.ports.len().max(1).ilog2() as u64 + 1)?;
        let index = self
            .ports
            .partition_point(|p| u64::from(p.address) + u64::from(p.width) <= u64::from(address));
        Ok(self
            .ports
            .get(index)
            .copied()
            .filter(|p| u64::from(p.address) < u64::from(address) + u64::from(width)))
    }
    /// Outer None means no device claims these bytes; inner None is a model gap.
    pub fn read(
        &mut self,
        address: u32,
        width: u8,
        c: &mut dyn RunControl,
    ) -> Result<Option<Option<u32>>> {
        let Some(port) = self.port(address, width, c)? else {
            return Ok(None);
        };
        let instance = self.instances[port.model].as_mut().unwrap();
        if address != port.address || width != port.width {
            instance.issue = Some(DeviceIssue::AccessWidth);
            return Ok(Some(None));
        }
        Ok(Some(instance.read(port.slot, c)?))
    }
    pub fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        c: &mut dyn RunControl,
    ) -> Result<Option<bool>> {
        let Some(port) = self.port(address, width, c)? else {
            return Ok(None);
        };
        let instance = self.instances[port.model].as_mut().unwrap();
        if address != port.address || width != port.width {
            instance.issue = Some(DeviceIssue::AccessWidth);
            return Ok(Some(false));
        }
        instance.write(port.slot, value, c).map(Some)
    }
    pub fn finish(
        &mut self,
        close_chain: bool,
        output: &mut Vec<ModelObservation>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        output.clear();
        for slot in &mut *self.instances {
            let Some(instance) = slot else {
                continue;
            };
            c.checkpoint(1)?;
            let closed = close_chain || instance.declaration.lifetime == RegionLifetime::Phase;
            output.push(instance.observation(closed)); // Session admits MAX_DEVICE_MODELS bounded rows.
            if closed {
                *slot = None;
            }
        }
        self.rebuild(c)
    }
}
impl<'m> Instance<'m> {
    fn new(
        declaration: &DeviceDeclaration,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        declaration.validate()?;
        let payload = memory.reserve(declaration.payload_bytes() + 64, c.position())?;
        let definition = declaration.identity(c)?;
        let mut instance = Self {
            declaration: declaration.clone(),
            commands: None,
            definition,
            _payload: payload,
            cells: AdmittedVec::new(memory),
            values: AdmittedVec::new(memory),
            value: 0,
            index: None,
            reads: 0,
            writes: 0,
            read_cursor: 0,
            write_cursor: 0,
            issue: None,
        };
        match &declaration.behavior {
            DeviceBehavior::CommandBank(bank) => {
                instance.commands = Some(crate::command_bank::CommandState::new(bank, memory, c)?);
            }
            DeviceBehavior::RegisterBank { cells } => {
                for cell in cells {
                    instance.cells.push(cell.clone(), c.position())?;
                }
            }
            DeviceBehavior::IndexedBank { values, index, .. } => {
                instance.index = *index;
                for value in values {
                    instance.values.push(*value, c.position())?;
                }
            }
            DeviceBehavior::W1c { initial, .. }
            | DeviceBehavior::ReadClear { initial, .. }
            | DeviceBehavior::SelfClearing { initial, .. } => instance.value = *initial,
            _ => {}
        }
        Ok(instance)
    }
    fn read(&mut self, slot: usize, c: &mut dyn RunControl) -> Result<Option<u32>> {
        c.checkpoint(1)?;
        let next = self.reads.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimited, "model read counter exhausted")
        })?;
        let value = match &self.declaration.behavior {
            DeviceBehavior::CommandBank(bank) => {
                Ok(self.commands.as_mut().unwrap().read(bank, slot)?)
            }
            DeviceBehavior::RegisterBank { .. } => Ok(self.cells[slot].value),
            DeviceBehavior::ConstantRead { value, .. } => Ok(*value),
            DeviceBehavior::SequenceRead { values, .. }
            | DeviceBehavior::Fifo { reads: values, .. } => match values.get(self.read_cursor) {
                Some(value) => {
                    self.read_cursor += 1;
                    Ok(*value)
                }
                None => Err(DeviceIssue::ExhaustedReads),
            },
            DeviceBehavior::W1c {
                read_clear_mask: mask,
                ..
            }
            | DeviceBehavior::ReadClear {
                clear_mask: mask, ..
            } => {
                let old = self.value;
                self.value &= !mask;
                Ok(old)
            }
            DeviceBehavior::SelfClearing { .. } => Ok(self.value),
            DeviceBehavior::IndexedBank { .. } => match self.index {
                Some(index) => Ok(if slot == 0 {
                    index
                } else {
                    self.values[index as usize]
                }),
                None => Err(DeviceIssue::UnknownIndex),
            },
        };
        match value {
            Ok(value) => {
                self.reads = next;
                Ok(Some(value))
            }
            Err(issue) => {
                self.issue = Some(issue);
                Ok(None)
            }
        }
    }
    fn write(&mut self, slot: usize, value: u32, c: &mut dyn RunControl) -> Result<bool> {
        c.checkpoint(1)?;
        let next = self.writes.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimited, "model write counter exhausted")
        })?;
        let result = match &self.declaration.behavior {
            DeviceBehavior::CommandBank(bank) => self
                .commands
                .as_mut()
                .unwrap()
                .write(bank, slot, value, c)?,
            DeviceBehavior::RegisterBank { .. } => {
                self.cells[slot].value = value;
                Ok(())
            }
            DeviceBehavior::ConstantRead { .. }
            | DeviceBehavior::SequenceRead { .. }
            | DeviceBehavior::ReadClear { .. } => Err(DeviceIssue::ReadOnly),
            DeviceBehavior::W1c { clear_mask, .. } => {
                self.value &= !(value & clear_mask);
                Ok(())
            }
            DeviceBehavior::SelfClearing { store_mask, .. } => {
                self.value = (self.value & !store_mask) | (value & store_mask);
                Ok(())
            }
            DeviceBehavior::Fifo { writes, .. } => match writes.get(self.write_cursor) {
                None => Err(DeviceIssue::UnexpectedWrite),
                Some(expected) if *expected != value => Err(DeviceIssue::WriteMismatch {
                    expected: *expected,
                    actual: value,
                }),
                Some(_) => {
                    self.write_cursor += 1;
                    Ok(())
                }
            },
            DeviceBehavior::IndexedBank { .. } => {
                if slot == 0 {
                    if value as usize >= self.values.len() {
                        Err(DeviceIssue::IndexOutOfRange { index: value })
                    } else {
                        self.index = Some(value);
                        Ok(())
                    }
                } else if let Some(index) = self.index {
                    self.values[index as usize] = value;
                    Ok(())
                } else {
                    Err(DeviceIssue::UnknownIndex)
                }
            }
        };
        match result {
            Ok(()) => {
                self.writes = next;
                Ok(true)
            }
            Err(issue) => {
                self.issue = Some(issue);
                Ok(false)
            }
        }
    }
    fn observation(&self, closed: bool) -> ModelObservation {
        let (reads, writes) = match &self.declaration.behavior {
            DeviceBehavior::SequenceRead { values, .. } => (values.len(), 0),
            DeviceBehavior::Fifo { reads, writes, .. } => (reads.len(), writes.len()),
            _ => (0, 0),
        };
        let mut result = ModelObservation {
            commands: self.commands.as_ref().map(|c| c.observation()),
            id: self.declaration.id.clone(),
            definition: self.definition.clone(),
            lifetime: self.declaration.lifetime,
            reads: self.reads,
            writes: self.writes,
            remaining_reads: self
                .commands
                .as_ref()
                .map_or((reads - self.read_cursor) as u32, |c| c.remaining()),
            remaining_writes: (writes - self.write_cursor) as u32,
            closed,
            issue: self.issue,
            status: ModelStatus::Open,
        };
        result.status = result.expected_status();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn declaration(lifetime: RegionLifetime) -> DeviceDeclaration {
        DeviceDeclaration {
            id: "large".into(),
            applicability: "synthetic lifecycle".into(),
            lifetime,
            behavior: DeviceBehavior::IndexedBank {
                index_address: 0x1000,
                data_address: 0x1004,
                width: 4,
                index: Some(0),
                values: vec![7; MAX_DEVICE_VALUES],
            },
        }
    }
    #[test]
    fn closure_releases_owned_payload_and_state_while_warm_keeps_them() {
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut devices = Devices::new(&memory);
        let mut output = Vec::with_capacity(MAX_DEVICE_MODELS);
        let mut c = || Ok(());
        devices
            .install(
                &[declaration(RegionLifetime::Phase)],
                &mut c,
                &mut |_, _, _| Ok(()),
            )
            .unwrap();
        let live = memory.observation().reserved_bytes;
        devices.finish(false, &mut output, &mut c).unwrap();
        let reusable = memory.observation().reserved_bytes;
        assert!(live > reusable + 2 * (MAX_DEVICE_VALUES * 4) as u64);
        assert!(!devices.overlaps(0x1000, 8, &mut c).unwrap());
        devices
            .install(
                &[declaration(RegionLifetime::Session)],
                &mut c,
                &mut |_, _, _| Ok(()),
            )
            .unwrap();
        let live = memory.observation().reserved_bytes;
        devices.write(0x1004, 4, 99, &mut c).unwrap();
        devices.finish(false, &mut output, &mut c).unwrap();
        assert_eq!(memory.observation().reserved_bytes, live);
        assert_eq!(output[0].status, ModelStatus::Open);
        assert_eq!(devices.read(0x1004, 4, &mut c).unwrap(), Some(Some(99)));
        devices.finish(true, &mut output, &mut c).unwrap();
        assert_eq!(memory.observation().reserved_bytes, reusable);
        assert_eq!(output[0].status, ModelStatus::Complete);
        drop(devices);
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
    #[test]
    fn failed_admission_releases_partial_state_and_cancelled_access_does_not_consume() {
        // Declaration clone fits, but its separate mutable bank cannot grow to full size.
        let memory = WorkingMemory::new(24 * 1024).unwrap();
        let mut devices = Devices::new(&memory);
        let error = devices
            .install(
                &[declaration(RegionLifetime::Session)],
                &mut || Ok(()),
                &mut |_, _, _| Ok(()),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!(memory.observation().reserved_bytes, 0);
        let mut d = declaration(RegionLifetime::Phase);
        d.behavior = DeviceBehavior::SequenceRead {
            address: 0x1000,
            width: 4,
            values: vec![42],
        };
        devices
            .install(&[d], &mut || Ok(()), &mut |_, _, _| Ok(()))
            .unwrap();
        let error = devices
            .read(0x1000, 4, &mut || {
                Err(Error::new(ErrorCode::Cancelled, "cancel fixture"))
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(
            devices.read(0x1000, 4, &mut || Ok(())).unwrap(),
            Some(Some(42))
        );
        drop(devices);
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
}
