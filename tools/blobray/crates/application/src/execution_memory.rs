//! Operation-owned mutable memory. All regions and event capacity die with the session.
use crate::*;
#[derive(Clone, Copy, Eq, PartialEq)]
enum RegionKind {
    Image,
    Stack,
    Ram,
}
struct Mapping {
    address: u32,
    length: usize,
    flags: u32,
    kind: RegionKind,
}
struct Region<'a> {
    address: u32,
    bytes: ScratchBytes<'a>,
    known: ScratchBytes<'a>,
    flags: u32,
    kind: RegionKind,
}
pub(super) struct Session<'a> {
    regions: Vec<Region<'a>>,
    memory: &'a WorkingMemory,
    _metadata: MemoryReservation<'a>,
    cells: Vec<RegisterCell>,
    events: Vec<ExecutionEvent>,
    max_events: usize,
}
impl<'a> Session<'a> {
    pub fn new(memory: &'a WorkingMemory, max_events: u32, c: &mut dyn RunControl) -> Result<Self> {
        let metadata = memory.reserve(1024 * 1024 + u64::from(max_events) * 128, c.position())?;
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(2048)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "region allocation refused"))?;
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(1024)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "MMIO allocation refused"))?;
        let mut events = Vec::new();
        events
            .try_reserve_exact(max_events as usize)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "event allocation refused"))?;
        Ok(Self {
            regions,
            memory,
            _metadata: metadata,
            cells,
            events,
            max_events: max_events as usize,
        })
    }
    fn region(
        &mut self,
        mapping: Mapping,
        fill: Option<u8>,
        bytes: &[u8],
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let Mapping {
            address,
            length,
            flags,
            kind,
        } = mapping;
        let end = u64::from(address) + length as u64;
        if self.regions.len() == 2048 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "session region capacity exhausted",
            ));
        }
        if end >= u64::from(u32::MAX - 1) || bytes.len() > length {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid session region",
            ));
        }
        for region in &self.regions {
            c.checkpoint(1)?;
            if u64::from(address) < u64::from(region.address) + region.bytes.len() as u64
                && u64::from(region.address) < end
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "session memory regions overlap",
                ));
            }
        }
        let mut data = self.memory.bytes(length, c.position())?;
        let mut known = self.memory.bytes(length, c.position())?;
        for offset in (0..length).step_by(WORK_BLOCK) {
            c.checkpoint(1)?;
            let end = (offset + WORK_BLOCK).min(length);
            data[offset..end].fill(fill.unwrap_or(0));
            known[offset..end].fill(u8::from(fill.is_some()));
        }
        for (chunk, source) in data.chunks_mut(WORK_BLOCK).zip(bytes.chunks(WORK_BLOCK)) {
            c.checkpoint(1)?;
            chunk[..source.len()].copy_from_slice(source);
        }
        for chunk in known[..bytes.len()].chunks_mut(WORK_BLOCK) {
            c.checkpoint(1)?;
            chunk.fill(1);
        }
        self.regions.push(Region {
            address,
            bytes: data,
            known,
            flags,
            kind,
        });
        Ok(())
    }
    pub fn image(&mut self, source: &dyn ByteSource, c: &mut dyn RunControl) -> Result<()> {
        blobray_artifacts::execution_segments(source, self.memory, c, &mut |segment, bytes, c| {
            self.region(
                Mapping {
                    address: segment.address as u32,
                    length: segment.memory_size as usize,
                    flags: segment.flags,
                    kind: RegionKind::Image,
                },
                Some(0),
                bytes,
                c,
            )
        })
    }
    pub fn phase(
        &mut self,
        target: &ExecutionTarget,
        input: &Invocation,
        c: &mut dyn RunControl,
    ) -> Result<u32> {
        self.regions.retain(|r| r.kind != RegionKind::Stack);
        self.events.clear();
        self.cells.clear();
        self.region(
            Mapping {
                address: target.stack.address,
                length: target.stack.length as usize,
                flags: 6,
                kind: RegionKind::Stack,
            },
            target.stack.fill,
            &target.stack.bytes,
            c,
        )?;
        for seed in &input.memory {
            c.checkpoint(self.regions.len() as u64)?;
            // Only a declared RAM region can be reseeded; never replace ELF data.
            if let Some(r) = self.regions.iter_mut().find(|r| {
                r.address == seed.address
                    && r.kind == RegionKind::Ram
                    && r.bytes.len() == seed.length as usize
            }) {
                for offset in (0..r.bytes.len()).step_by(WORK_BLOCK) {
                    c.checkpoint(1)?;
                    let end = (offset + WORK_BLOCK).min(r.bytes.len());
                    if let Some(fill) = seed.fill {
                        r.bytes[offset..end].fill(fill);
                        r.known[offset..end].fill(1);
                    }
                }
                for (dst, src) in r
                    .bytes
                    .chunks_mut(WORK_BLOCK)
                    .zip(seed.bytes.chunks(WORK_BLOCK))
                {
                    c.checkpoint(1)?;
                    dst[..src.len()].copy_from_slice(src);
                }
                for chunk in r.known[..seed.bytes.len()].chunks_mut(WORK_BLOCK) {
                    c.checkpoint(1)?;
                    chunk.fill(1);
                }
            } else {
                self.region(
                    Mapping {
                        address: seed.address,
                        length: seed.length as usize,
                        flags: 6,
                        kind: RegionKind::Ram,
                    },
                    seed.fill,
                    &seed.bytes,
                    c,
                )?;
            }
        }
        for cell in &input.mmio {
            c.checkpoint((self.regions.len() + self.cells.len() + 1) as u64)?;
            let start = u64::from(cell.address);
            let end = start + u64::from(cell.width);
            if self.regions.iter().any(|r| {
                start < u64::from(r.address) + r.bytes.len() as u64 && u64::from(r.address) < end
            }) || self.cells.iter().any(|r| {
                start < u64::from(r.address) + u64::from(r.width) && u64::from(r.address) < end
            }) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "MMIO cells overlap memory or one another",
                ));
            }
            self.cells.push(cell.clone());
        }
        Ok(target.stack.address + target.stack.length)
    }
    pub fn observation(&mut self, stop: ExecutionStop, steps: u64) -> ExecutionObservation {
        ExecutionObservation {
            stop,
            steps,
            events: std::mem::take(&mut self.events),
        }
    }
    pub fn recycle(&mut self, mut observation: ExecutionObservation) {
        observation.events.clear();
        self.events = observation.events;
    }
    fn region_index(&self, address: u32, width: u8) -> Option<(usize, usize)> {
        self.regions.iter().enumerate().find_map(|(i, r)| {
            let offset = u64::from(address).checked_sub(u64::from(r.address))?;
            (offset + u64::from(width) <= r.bytes.len() as u64).then_some((i, offset as usize))
        })
    }
}
impl ExecutionMemory for Session<'_> {
    fn read(
        &mut self,
        address: u32,
        width: u8,
        access: MemoryAccess,
        c: &mut dyn RunControl,
    ) -> Result<Option<u32>> {
        c.checkpoint((self.regions.len() + self.cells.len() + 1) as u64)?;
        if !matches!(width, 1 | 2 | 4) || !address.is_multiple_of(u32::from(width)) {
            return Ok(None);
        }
        if access == MemoryAccess::Read
            && let Some(cell) = self
                .cells
                .iter()
                .find(|cell| cell.address == address && cell.width == width)
        {
            let value = cell.value;
            self.event(
                ExecutionEvent::Read {
                    address,
                    width,
                    value,
                },
                c,
            )?;
            return Ok(Some(value));
        }
        let Some((i, offset)) = self.region_index(address, width) else {
            return Ok(None);
        };
        let r = &self.regions[i];
        let required = if access == MemoryAccess::Fetch { 1 } else { 4 };
        if r.flags & required == 0 || r.known[offset..offset + width as usize].contains(&0) {
            return Ok(None);
        }
        let mut bytes = [0; 4];
        bytes[..width as usize].copy_from_slice(&r.bytes[offset..offset + width as usize]);
        Ok(Some(u32::from_le_bytes(bytes)))
    }
    fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        c.checkpoint((self.regions.len() + self.cells.len() + 1) as u64)?;
        if !matches!(width, 1 | 2 | 4) || !address.is_multiple_of(u32::from(width)) {
            return Ok(false);
        }
        let value = if width == 4 {
            value
        } else {
            value & ((1 << (width * 8)) - 1)
        };
        if let Some(index) = self
            .cells
            .iter()
            .position(|cell| cell.address == address && cell.width == width)
        {
            self.event(
                ExecutionEvent::Write {
                    address,
                    width,
                    value,
                },
                c,
            )?;
            self.cells[index].value = value;
            return Ok(true);
        }
        let Some((i, offset)) = self.region_index(address, width) else {
            return Ok(false);
        };
        let r = &mut self.regions[i];
        if r.flags & 2 == 0 {
            return Ok(false);
        }
        r.bytes[offset..offset + width as usize]
            .copy_from_slice(&value.to_le_bytes()[..width as usize]);
        r.known[offset..offset + width as usize].fill(1);
        Ok(true)
    }
    fn event(&mut self, event: ExecutionEvent, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        if self.events.len() == self.max_events {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "execution event capacity exhausted",
            ));
        }
        self.events.push(event);
        Ok(())
    }
}
