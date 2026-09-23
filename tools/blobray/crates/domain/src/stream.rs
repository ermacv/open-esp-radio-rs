//! Narrow streaming ports. Computing code receives bytes, never origin paths.
use crate::*;

/// Stable captured content with positional, bounded reads. Implementations must
/// reject overflow/short reads; callers own the destination's memory reservation.
pub trait ByteSource {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8], control: &mut dyn RunControl) -> Result<()>;
}

pub fn hash_source(source: &dyn ByteSource, control: &mut dyn RunControl) -> Result<ArtifactId> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut buffer = [0; WORK_BLOCK];
    let mut offset = 0;
    while offset < source.len() {
        let size = (source.len() - offset).min(WORK_BLOCK as u64) as usize;
        source.read_at(offset, &mut buffer[..size], control)?;
        control.bytes(size)?;
        hash.update(&buffer[..size]);
        offset += size as u64;
    }
    format!("{:x}", hash.finalize()).parse()
}

pub struct SourceRange<'a> {
    source: &'a dyn ByteSource,
    offset: u64,
    length: u64,
}
impl<'a> SourceRange<'a> {
    pub fn new(source: &'a dyn ByteSource, offset: u64, length: u64) -> Result<Self> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > source.len())
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "payload range outside captured content",
            ));
        }
        Ok(Self {
            source,
            offset,
            length,
        })
    }
}
impl ByteSource for SourceRange<'_> {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8], control: &mut dyn RunControl) -> Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "read outside payload range",
            ));
        }
        self.source.read_at(self.offset + offset, bytes, control)
    }
}

pub fn read_scratch<'a>(
    source: &dyn ByteSource,
    memory: &'a WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<ScratchBytes<'a>> {
    let length = usize::try_from(source.len()).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "payload length exceeds host address space",
        )
    })?;
    let mut bytes = memory.bytes(length, control.position())?;
    source.read_at(0, &mut bytes, control)?;
    Ok(bytes)
}

/// A synchronous consumer must finish using each borrowed record before return.
/// Retaining a clone requires the consumer's own admitted memory capacity.
pub trait ElfSink {
    fn section(&mut self, record: &SectionRecord, control: &mut dyn RunControl) -> Result<()>;
    fn symbol(&mut self, record: &SymbolRecord, control: &mut dyn RunControl) -> Result<()>;
    fn relocation(&mut self, record: &RelocationRecord, control: &mut dyn RunControl)
    -> Result<()>;
    fn diagnostic(&mut self, record: &Diagnostic, control: &mut dyn RunControl) -> Result<()>;
}

/// Snapshot callbacks have read-only authority and preserve physical order.
pub trait InventorySink: ElfSink {
    /// Container framing coverage for the current input, before object callbacks.
    fn container(
        &mut self,
        _kind: ContainerKind,
        _members_complete: bool,
        _control: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }

    fn revision(&mut self, _header: &RevisionHeader, _control: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn input(
        &mut self,
        _ordinal: u64,
        _input: &InputRecord,
        _control: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
    fn object(&mut self, _object: &ObjectInventory, _control: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn external(&mut self, _member: &ExternalMember, _control: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
impl ElfSink for () {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
impl InventorySink for () {}

impl ByteSource for &[u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_at(
        &self,
        offset: u64,
        destination: &mut [u8],
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let end = offset
            .checked_add(destination.len() as u64)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "borrowed range overflow"))?;
        let start = usize::try_from(offset)
            .map_err(|_| Error::new(ErrorCode::Integrity, "borrowed offset overflow"))?;
        let end = usize::try_from(end)
            .map_err(|_| Error::new(ErrorCode::Integrity, "borrowed extent overflow"))?;
        let source = self
            .get(start..end)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "borrowed range outside content"))?;
        for (source, destination) in source
            .chunks(WORK_BLOCK)
            .zip(destination.chunks_mut(WORK_BLOCK))
        {
            control.bytes(source.len())?;
            destination.copy_from_slice(source);
        }
        Ok(())
    }
}
