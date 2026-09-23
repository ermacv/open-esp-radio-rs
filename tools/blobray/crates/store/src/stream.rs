//! Staged, schema-1 JSON assembly. Each table lives on disk while it is produced.
use super::metered::{hash_file, write_json};
use super::*;
use std::io::{Read, Seek, SeekFrom, Write};

fn raw(file: &mut TemporaryFile, bytes: &[u8], control: &mut dyn RunControl) -> Result<()> {
    for chunk in bytes.chunks(WORK_BLOCK) {
        control.bytes(chunk.len())?;
        file.write_all(chunk).map_err(io)?;
    }
    Ok(())
}
fn field<T: serde::Serialize>(
    file: &mut TemporaryFile,
    key: &[u8],
    value: &T,
    control: &mut dyn RunControl,
) -> Result<()> {
    raw(file, key, control)?;
    write_json(file, value, control)
}
fn temporary(stage: &Staging) -> Result<TemporaryFile> {
    stage.disk.temporary(&stage.root.join("staging"))
}
fn copy(
    source: &mut TemporaryFile,
    destination: &mut TemporaryFile,
    control: &mut dyn RunControl,
) -> Result<()> {
    source.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut remaining = source.metadata().map_err(io)?.len();
    let mut buffer = [0; WORK_BLOCK];
    while remaining != 0 {
        let count = remaining.min(WORK_BLOCK as u64) as usize;
        control.bytes(count)?;
        source.read_exact(&mut buffer[..count]).map_err(io)?;
        raw(destination, &buffer[..count], control)?;
        remaining -= count as u64;
    }
    Ok(())
}
struct Records {
    file: TemporaryFile,
    count: u64,
}
impl Records {
    fn new(stage: &Staging) -> Result<Self> {
        Ok(Self {
            file: temporary(stage)?,
            count: 0,
        })
    }
    fn prefix(&mut self, control: &mut dyn RunControl) -> Result<()> {
        control.checkpoint(1)?;
        if self.count != 0 {
            raw(self.file.as_file_mut(), b",", control)?;
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| integrity("record count overflow"))?;
        Ok(())
    }
    fn push<T: serde::Serialize>(
        &mut self,
        record: &T,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        self.prefix(control)?;
        write_json(self.file.as_file_mut(), record, control)
    }
    fn array(
        &mut self,
        destination: &mut TemporaryFile,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        raw(destination, b"[", control)?;
        copy(self.file.as_file_mut(), destination, control)?;
        raw(destination, b"]", control)
    }
}

/// No publication authority. Drop removes unfinished fragments on all error paths.
pub struct RevisionStream {
    stage: Staging,
    manifest: TemporaryFile,
    closure: TemporaryFile,
    project: ProjectId,
    parent: Option<RevisionId>,
    inputs: u64,
    complete: bool,
}
pub struct InputStream {
    metadata: InputRecord,
    kind: ContainerKind,
    external: Records,
    objects: Records,
    complete: bool,
}
pub struct ObjectStream {
    sections: Records,
    symbols: Records,
    relocations: Records,
    diagnostics: Records,
}
/// Borrowed object header; record tables arrive through `ObjectStream`.
pub struct ObjectHeader<'a> {
    pub id: &'a ObjectId,
    pub name: Option<&'a [u8]>,
    pub content: Option<&'a ArtifactId>,
    pub elf: Option<&'a ElfInventory>,
}
impl Staging {
    pub fn stream_revision(
        &self,
        project: ProjectId,
        parent: Option<RevisionId>,
        target: Target,
        producer: &str,
        control: &mut dyn RunControl,
    ) -> Result<RevisionStream> {
        let mut manifest = temporary(self)?;
        let file = manifest.as_file_mut();
        field(file, b"{\"schema\":", &1, control)?;
        field(file, b",\"project\":", &project, control)?;
        field(file, b",\"parent\":", &parent, control)?;
        field(file, b",\"target\":", &target, control)?;
        field(file, b",\"inventory_producer\":", &producer, control)?;
        raw(file, b",\"inputs\":[", control)?;
        Ok(RevisionStream {
            stage: Staging {
                root: self.root.clone(),
                disk: self.disk.clone(),
            },
            manifest,
            closure: temporary(self)?,
            project,
            parent,
            inputs: 0,
            complete: true,
        })
    }
}
impl RevisionStream {
    pub fn begin_input(
        &mut self,
        metadata: InputRecord,
        kind: ContainerKind,
        control: &mut dyn RunControl,
    ) -> Result<InputStream> {
        if metadata.role.trim().is_empty()
            || metadata.inventory.is_some()
            || !metadata.external_members.is_empty()
        {
            return Err(integrity("invalid streaming input header"));
        }
        if let (Some(expected), Some(actual)) = (&metadata.expected, metadata.capture.artifact())
            && expected != actual
        {
            return Err(integrity("input digest mismatch"));
        }
        self.capture(&metadata.capture, control)?;
        Ok(InputStream {
            metadata,
            kind,
            external: Records::new(&self.stage)?,
            objects: Records::new(&self.stage)?,
            complete: true,
        })
    }
    fn capture(&mut self, capture: &Capture, control: &mut dyn RunControl) -> Result<()> {
        if let Capture::Captured { artifact, length } = capture {
            let lease = self.stage.open_payload(artifact, control)?;
            if lease.len() != *length {
                return Err(integrity("capture length mismatch"));
            }
            write_json(self.closure.as_file_mut(), capture, control)?;
            raw(self.closure.as_file_mut(), b"\n", control)?;
        }
        Ok(())
    }
    pub fn bind_member(
        &mut self,
        input: &mut InputStream,
        member: &ExternalMember,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if input.kind != ContainerKind::ThinArchive || member.ordinal != input.external.count {
            return Err(integrity("thin binding order mismatch"));
        }
        self.capture(&member.capture, control)?;
        input.external.push(member, control)
    }
    pub fn begin_object(&self) -> Result<ObjectStream> {
        Ok(ObjectStream {
            sections: Records::new(&self.stage)?,
            symbols: Records::new(&self.stage)?,
            relocations: Records::new(&self.stage)?,
            diagnostics: Records::new(&self.stage)?,
        })
    }
    pub fn finish_object(
        &self,
        input: &mut InputStream,
        mut object: ObjectStream,
        header: ObjectHeader<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let ObjectHeader {
            id,
            name,
            content,
            elf,
        } = header;
        if elf.is_some_and(|header| {
            !header.sections.is_empty()
                || !header.symbols.is_empty()
                || !header.relocations.is_empty()
        }) {
            return Err(integrity(
                "streaming ELF header must not contain materialized tables",
            ));
        }
        let location = match input.kind {
            ContainerKind::Archive | ContainerKind::ThinArchive => ObjectLocation::ArchiveMember {
                ordinal: input.objects.count,
            },
            _ => ObjectLocation::Standalone,
        };
        if Some(&id.artifact) != input.metadata.capture.artifact()
            || id.location != location
            || (elf.is_some() && content.is_none())
        {
            return Err(integrity("object ownership mismatch"));
        }
        if location == ObjectLocation::Standalone && content != Some(&id.artifact) {
            return Err(integrity("standalone content mismatch"));
        }
        input.complete &= elf.is_some() && object.diagnostics.count == 0;
        input.objects.prefix(control)?;
        let file = input.objects.file.as_file_mut();
        field(file, b"{\"id\":", id, control)?;
        field(file, b",\"name\":", &name, control)?;
        field(file, b",\"content\":", &content, control)?;
        raw(file, b",\"elf\":", control)?;
        if let Some(elf) = elf {
            field(file, b"{\"bits\":", &elf.bits, control)?;
            field(file, b",\"little_endian\":", &elf.little_endian, control)?;
            field(file, b",\"machine\":", &elf.machine, control)?;
            field(file, b",\"object_type\":", &elf.object_type, control)?;
            field(file, b",\"entry\":", &elf.entry, control)?;
            raw(file, b",\"sections\":", control)?;
            object.sections.array(file, control)?;
            raw(file, b",\"symbols\":", control)?;
            object.symbols.array(file, control)?;
            raw(file, b",\"relocations\":", control)?;
            object.relocations.array(file, control)?;
            raw(file, b"}", control)?;
        } else {
            raw(file, b"null", control)?;
        }
        raw(file, b",\"diagnostics\":", control)?;
        object.diagnostics.array(file, control)?;
        raw(file, b"}", control)
    }
    pub fn finish_input(
        &mut self,
        mut input: InputStream,
        members_complete: bool,
        diagnostic: Option<&Diagnostic>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let available = input.metadata.capture.artifact().is_some();
        if input.kind == ContainerKind::ThinArchive && input.objects.count != input.external.count {
            return Err(integrity("thin bindings do not cover objects"));
        }
        if available
            && !matches!(
                input.kind,
                ContainerKind::Archive | ContainerKind::ThinArchive
            )
            && input.objects.count != 1
        {
            return Err(integrity("standalone input requires one outcome"));
        }
        self.complete &= available && input.complete && members_complete && diagnostic.is_none();
        if self.inputs != 0 {
            raw(self.manifest.as_file_mut(), b",", control)?;
        }
        self.inputs += 1;
        let file = self.manifest.as_file_mut();
        field(file, b"{\"role\":", &input.metadata.role, control)?;
        field(file, b",\"origin\":", &input.metadata.origin, control)?;
        field(file, b",\"expected\":", &input.metadata.expected, control)?;
        field(file, b",\"capture\":", &input.metadata.capture, control)?;
        raw(file, b",\"external_members\":", control)?;
        input.external.array(file, control)?;
        raw(file, b",\"inventory\":", control)?;
        if available {
            field(file, b"{\"kind\":", &input.kind, control)?;
            field(file, b",\"members_complete\":", &members_complete, control)?;
            raw(file, b",\"objects\":", control)?;
            input.objects.array(file, control)?;
            raw(file, b",\"diagnostics\":[", control)?;
            if let Some(diagnostic) = diagnostic {
                write_json(file, diagnostic, control)?;
            }
            raw(file, b"]}", control)?;
        } else {
            raw(file, b"null", control)?;
        }
        raw(file, b"}", control)
    }
    pub fn finish(
        mut self,
        memory: &WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<PreparedImport> {
        if self.inputs == 0 {
            return Err(integrity("revision requires inputs"));
        }
        raw(self.manifest.as_file_mut(), b"]}", control)?;
        self.manifest.as_file().sync_all().map_err(io)?;
        self.closure.as_file().sync_all().map_err(io)?;
        let revision = hash_file(self.manifest.path(), control)?.0;
        let closure = hash_file(self.closure.path(), control)?.0;
        let lease = FileLease::open(self.manifest.path(), &revision, control)?;
        let _fixed = memory.reserve(32768, control.position())?;
        let reader = Project {
            root: self.stage.root.clone(),
            id: self.project.clone(),
        };
        let complete = reader.walk(&lease, memory, control, &mut ())?;
        if complete != self.complete {
            return Err(integrity("streamed completeness disagrees with manifest"));
        }

        self.stage.persist(self.manifest, &revision, control)?;
        self.stage.persist(self.closure, &closure, control)?;
        Ok(PreparedImport {
            schema: 1,
            project: self.project,
            parent: self.parent,
            revision: revision.as_str().parse()?,
            closure,
            complete: self.complete,
        })
    }
}
impl ElfSink for ObjectStream {
    fn section(&mut self, record: &SectionRecord, control: &mut dyn RunControl) -> Result<()> {
        self.sections.push(record, control)
    }
    fn symbol(&mut self, record: &SymbolRecord, control: &mut dyn RunControl) -> Result<()> {
        self.symbols.push(record, control)
    }
    fn relocation(
        &mut self,
        record: &RelocationRecord,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        self.relocations.push(record, control)
    }
    fn diagnostic(&mut self, record: &Diagnostic, control: &mut dyn RunControl) -> Result<()> {
        self.diagnostics.push(record, control)
    }
}
