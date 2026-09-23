//! Read-only validation and record delivery with no materialized revision graph.
use super::json_ranges::{Json, Node};
use super::*;

pub struct SnapshotView {
    pub revision_id: RevisionId,
    pub complete: bool,
    manifest: FileLease,
}
impl SnapshotView {
    /// Raw schema-1 manifest, already validated. The lease never opens origins.
    pub fn manifest(&self) -> &dyn ByteSource {
        &self.manifest
    }
}
impl Project {
    pub fn read_inventory(
        &self,
        requested: Option<&RevisionId>,
        memory: &WorkingMemory,
        control: &mut dyn RunControl,
        sink: &mut dyn InventorySink,
    ) -> Result<SnapshotView> {
        let id = match requested {
            Some(id) => id.clone(),
            None => self.current()?.ok_or_else(|| {
                Error::new(ErrorCode::NotFound, "project has no imported revision")
            })?,
        };
        let connection = open_connection(&self.root, false)?;
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=?1)",
                [id.as_str()],
                |row| row.get(0),
            )
            .map_err(db)?;
        if !exists {
            return Err(Error::new(
                ErrorCode::NotFound,
                "revision does not belong to this project",
            ));
        }
        let manifest = self.open_payload(&id.as_str().parse()?, control)?;
        let _fixed = memory.reserve(32768, control.position())?;
        let complete = self.walk(&manifest, memory, control, sink)?;
        Ok(SnapshotView {
            revision_id: id,
            complete,
            manifest,
        })
    }
    fn verify_capture(&self, capture: &Capture, control: &mut dyn RunControl) -> Result<()> {
        if let Capture::Captured { artifact, length } = capture {
            let lease = self.open_payload(artifact, control)?;
            if lease.len() != *length {
                return Err(integrity("capture length differs from retained payload"));
            }
        }
        Ok(())
    }
    pub(crate) fn walk(
        &self,
        manifest: &dyn ByteSource,
        memory: &WorkingMemory,
        control: &mut dyn RunControl,
        sink: &mut dyn InventorySink,
    ) -> Result<bool> {
        walk_manifest(
            &self.id,
            manifest,
            memory,
            control,
            sink,
            &mut |capture, control| self.verify_capture(capture, control),
        )
    }
}
/// An identified manifest-only lease. It validates records but does not retain
/// or verify the payload closure. Suitable for inspection of captured metadata.
pub struct ManifestLease {
    project: ProjectId,
    manifest: FileLease,
}
impl ManifestLease {
    pub fn open(
        path: &Path,
        project: ProjectId,
        revision: &RevisionId,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        Ok(Self {
            project,
            manifest: FileLease::open(path, &revision.as_str().parse()?, control)?,
        })
    }
    pub fn visit(
        &self,
        memory: &WorkingMemory,
        control: &mut dyn RunControl,
        sink: &mut dyn InventorySink,
    ) -> Result<bool> {
        let _fixed = memory.reserve(32768, control.position())?;
        walk_manifest(
            &self.project,
            &self.manifest,
            memory,
            control,
            sink,
            &mut |_, _| Ok(()),
        )
    }
    pub fn manifest(&self) -> &dyn ByteSource {
        &self.manifest
    }
}
fn walk_manifest(
    project_id: &ProjectId,
    manifest: &dyn ByteSource,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn InventorySink,
    verify_capture: &mut dyn FnMut(&Capture, &mut dyn RunControl) -> Result<()>,
) -> Result<bool> {
    control.phase(RunPhase::ValidateRevision)?;
    let mut json = Json::new(manifest);
    let root = json.root(control)?;
    let [schema, project, parent, target, producer, inputs] = json.fields(
        root,
        [
            "schema",
            "project",
            "parent",
            "target",
            "inventory_producer",
            "inputs",
        ],
        control,
    )?;
    if *json.decode::<u32>(schema, memory, control)? != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported revision schema",
        ));
    }
    if *json.decode::<ProjectId>(project, memory, control)? != *project_id {
        return Err(integrity("manifest belongs to another project"));
    }
    let parent = json.decode::<Option<RevisionId>>(parent, memory, control)?;
    let target = json.decode::<Target>(target, memory, control)?;
    let producer = json.decode::<String>(producer, memory, control)?;
    if producer.is_empty() {
        return Err(integrity("inventory producer missing"));
    }
    sink.revision(
        &RevisionHeader {
            project: project_id.clone(),
            parent: parent.clone(),
            target: *target,
            inventory_producer: producer.clone(),
        },
        control,
    )?;
    let mut inputs = json.array(inputs)?;
    let mut count = 0;
    let mut complete = true;
    while let Some(input) = json.next(&mut inputs, control)? {
        control.set_position(RunPosition {
            phase: RunPhase::ValidateRevision,
            input: Some(count),
            ..Default::default()
        });
        let [role, origin, expected, capture, external, inventory] = json.fields(
            input,
            [
                "role",
                "origin",
                "expected",
                "capture",
                "external_members",
                "inventory",
            ],
            control,
        )?;
        let role = json.decode::<String>(role, memory, control)?;
        let origin = json.decode::<OriginPath>(origin, memory, control)?;
        let expected = json.decode::<Option<ArtifactId>>(expected, memory, control)?;
        let capture = json.decode::<Capture>(capture, memory, control)?;
        if role.trim().is_empty() {
            return Err(integrity("input role is empty"));
        }
        if let (Some(expected), Some(actual)) = (&*expected, capture.artifact())
            && expected != actual
        {
            return Err(integrity("input digest differs from expectation"));
        }
        verify_capture(&capture, control)?;
        sink.input(
            count,
            &InputRecord {
                role: role.clone(),
                origin: origin.clone(),
                expected: expected.clone(),
                capture: capture.clone(),
                external_members: Vec::new(),
                inventory: None,
            },
            control,
        )?;
        let mut external = json.array(external)?;
        if capture.artifact().is_none() {
            json.decode::<()>(inventory, memory, control)?;
            if json.next(&mut external, control)?.is_some() {
                return Err(integrity("unavailable input has external bindings"));
            }
            complete = false;
            count += 1;
            continue;
        }
        let artifact = capture.artifact().unwrap();
        let [kind, members_complete, objects, diagnostics] = json.fields(
            inventory,
            ["kind", "members_complete", "objects", "diagnostics"],
            control,
        )?;
        let kind = *json.decode::<ContainerKind>(kind, memory, control)?;
        let members_complete = *json.decode::<bool>(members_complete, memory, control)?;
        complete &= members_complete;
        sink.container(kind, members_complete, control)?;
        complete &= diagnostics_empty(&mut json, diagnostics, memory, control, sink)?;
        let mut objects = json.array(objects)?;
        let mut ordinal = 0;
        while let Some(object) = json.next(&mut objects, control)? {
            let mut position = control.position();
            position.member = Some(ordinal);
            position.artifact(artifact);
            control.set_position(position);
            let [id, name, content, elf, diagnostics] = json.fields(
                object,
                ["id", "name", "content", "elf", "diagnostics"],
                control,
            )?;
            let id = json.decode::<ObjectId>(id, memory, control)?;
            let name = json.decode::<Option<Vec<u8>>>(name, memory, control)?;
            let content = json.decode::<Option<ArtifactId>>(content, memory, control)?;
            let location = match kind {
                ContainerKind::Archive | ContainerKind::ThinArchive => {
                    ObjectLocation::ArchiveMember { ordinal }
                }
                _ => ObjectLocation::Standalone,
            };
            if id.artifact != *artifact || id.location != location {
                return Err(integrity("object identity outside its ordered input"));
            }
            if location == ObjectLocation::Standalone && content.as_ref() != Some(artifact) {
                return Err(integrity("standalone content identity mismatch"));
            }
            if kind == ContainerKind::ThinArchive {
                let member = json
                    .next(&mut external, control)?
                    .ok_or_else(|| integrity("missing thin binding"))?;
                let member = json.decode::<ExternalMember>(member, memory, control)?;
                if member.ordinal != ordinal
                    || name.as_ref() != Some(&member.name)
                    || member.capture.artifact() != content.as_ref()
                {
                    return Err(integrity("thin binding and object disagree"));
                }
                verify_capture(&member.capture, control)?;
                sink.external(&member, control)?;
            }
            let mut outcome = ObjectInventory {
                id: id.clone(),
                name: name.clone(),
                content: content.clone(),
                elf: None,
                diagnostics: Vec::new(),
            };
            if elf.kind == b'n' {
                json.decode::<()>(elf, memory, control)?;
                complete = false;
                sink.object(&outcome, control)?;
            } else {
                if content.is_none() {
                    return Err(integrity("ELF inventory without captured content"));
                }
                let [
                    bits,
                    little,
                    machine,
                    kind,
                    entry,
                    sections,
                    symbols,
                    relocations,
                ] = json.fields(
                    elf,
                    [
                        "bits",
                        "little_endian",
                        "machine",
                        "object_type",
                        "entry",
                        "sections",
                        "symbols",
                        "relocations",
                    ],
                    control,
                )?;
                outcome.elf = Some(ElfInventory {
                    bits: *json.decode(bits, memory, control)?,
                    little_endian: *json.decode(little, memory, control)?,
                    machine: *json.decode(machine, memory, control)?,
                    object_type: *json.decode(kind, memory, control)?,
                    entry: *json.decode(entry, memory, control)?,
                    sections: Vec::new(),
                    symbols: Vec::new(),
                    relocations: Vec::new(),
                });
                sink.object(&outcome, control)?;
                let mut sections = json.array(sections)?;
                while let Some(record) = json.next(&mut sections, control)? {
                    let record = json.decode::<SectionRecord>(record, memory, control)?;
                    sink.section(&record, control)?;
                }
                let beginning = json.array(symbols)?;
                let mut symbols = beginning;
                let mut seen = 0u64;
                let mut maximum = None;
                while let Some(record) = json.next(&mut symbols, control)? {
                    let record = json.decode::<SymbolRecord>(record, memory, control)?;
                    let key = (record.id.table_section, record.id.index);
                    let mut position = control.position();
                    position.table = Some(key.0 as u64);
                    position.entry = Some(key.1);
                    control.set_position(position);
                    if record.id.object != *id {
                        return Err(integrity("symbol belongs to another object"));
                    }
                    // Normal producer order is linear. Older valid unordered
                    // manifests remain accepted, with budgeted lookback on disk.
                    if maximum.is_some_and(|maximum| key <= maximum) {
                        let mut previous = beginning;
                        for _ in 0..seen {
                            let previous = json
                                .next(&mut previous, control)?
                                .ok_or_else(|| integrity("symbol table changed"))?;
                            let previous =
                                json.decode::<SymbolRecord>(previous, memory, control)?;
                            if (previous.id.table_section, previous.id.index) == key {
                                return Err(integrity("duplicate symbol identity"));
                            }
                        }
                    }
                    maximum = Some(maximum.map_or(key, |old| old.max(key)));
                    seen += 1;
                    sink.symbol(&record, control)?;
                }
                let mut relocations = json.array(relocations)?;
                while let Some(record) = json.next(&mut relocations, control)? {
                    let record = json.decode::<RelocationRecord>(record, memory, control)?;
                    sink.relocation(&record, control)?;
                }
            }
            complete &= diagnostics_empty(&mut json, diagnostics, memory, control, sink)?;
            ordinal += 1;
        }
        if json.next(&mut external, control)?.is_some() {
            return Err(integrity("unexpected external binding"));
        }
        if !matches!(kind, ContainerKind::Archive | ContainerKind::ThinArchive) && ordinal != 1 {
            return Err(integrity("standalone input requires one object"));
        }
        count += 1;
    }
    if count == 0 {
        return Err(integrity("revision requires inputs"));
    }
    Ok(complete)
}

fn diagnostics_empty(
    json: &mut Json<'_>,
    node: Node,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn InventorySink,
) -> Result<bool> {
    let mut array = json.array(node)?;
    let mut empty = true;
    while let Some(node) = json.next(&mut array, control)? {
        let record = json.decode::<Diagnostic>(node, memory, control)?;
        sink.diagnostic(&record, control)?;
        empty = false;
    }
    Ok(empty)
}

#[derive(serde::Serialize)]
pub struct DoctorSummary {
    pub schema: u32,
    pub project: ProjectId,
    pub storage_schema: u32,
    pub checked_revisions: u64,
    pub checked_images: u64,
    pub checked_analyses: u64,
    pub checked_publications: u64,
    pub errors: u64,
    pub unfinished_runs: u64,
}
pub trait DoctorSink {
    fn error(&mut self, error: &Error, control: &mut dyn RunControl) -> Result<()>;
    fn unfinished(&mut self, id: &RunId, control: &mut dyn RunControl) -> Result<()>;
}
impl Project {
    pub fn doctor_stream(
        &self,
        memory: &WorkingMemory,
        control: &mut dyn RunControl,
        sink: &mut dyn DoctorSink,
    ) -> Result<DoctorSummary> {
        let connection = open_connection(&self.root, false)?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        let mut report = DoctorSummary {
            schema: 1,
            project: self.id.clone(),
            storage_schema: schema,
            checked_revisions: 0,
            checked_images: 0,
            checked_analyses: 0,
            checked_publications: 0,
            errors: 0,
            unfinished_runs: 0,
        };
        let mut statement = connection
            .prepare("SELECT id FROM revisions ORDER BY sequence")
            .map_err(db)?;
        let mut rows = statement.query([]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            let id: String = row.get(0).map_err(db)?;
            let result = id
                .parse()
                .and_then(|id| self.read_inventory(Some(&id), memory, control, &mut ()));
            match result {
                Ok(_) => report.checked_revisions += 1,
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::ResourceLimited
                            | ErrorCode::TimedOut
                            | ErrorCode::Cancelled
                            | ErrorCode::DiagnosticChannel
                    ) =>
                {
                    return Err(error);
                }
                Err(error) => {
                    sink.error(&error, control)?;
                    report.errors += 1;
                }
            }
        }
        if self.current()?.is_some() {
            match self.read_inventory(None, memory, control, &mut ()) {
                Ok(_) => (),
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::ResourceLimited
                            | ErrorCode::TimedOut
                            | ErrorCode::Cancelled
                            | ErrorCode::DiagnosticChannel
                    ) =>
                {
                    return Err(error);
                }
                Err(error) => {
                    sink.error(&error, control)?;
                    report.errors += 1;
                }
            }
        }
        if schema >= 3 {
            self.images(control, &mut |id, control| {
                let _metadata = memory.reserve(8 * 1024 * 1024, control.position())?;
                match self.image(id, control) {
                    Ok(_) => report.checked_images += 1,
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::ResourceLimited
                                | ErrorCode::TimedOut
                                | ErrorCode::Cancelled
                                | ErrorCode::DiagnosticChannel
                        ) =>
                    {
                        return Err(error);
                    }
                    Err(error) => {
                        sink.error(&error, control)?;
                        report.errors += 1;
                    }
                }
                Ok(())
            })?;
        }
        if schema >= 4 {
            self.analyses(control, &mut |id, control| {
                let _metadata = memory.reserve(8 * 1024 * 1024, control.position())?;
                match self.analysis(id, control) {
                    Ok(_) => report.checked_analyses += 1,
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::ResourceLimited
                                | ErrorCode::TimedOut
                                | ErrorCode::Cancelled
                                | ErrorCode::DiagnosticChannel
                        ) =>
                    {
                        return Err(error);
                    }
                    Err(error) => {
                        sink.error(&error, control)?;
                        report.errors += 1;
                    }
                }
                Ok(())
            })?;
        }
        if schema >= 5 {
            self.publications(control, &mut |id, control| {
                let _metadata = memory.reserve(8 * 1024 * 1024, control.position())?;
                match self.publication(id, control) {
                    Ok(_) => report.checked_publications += 1,
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::ResourceLimited
                                | ErrorCode::TimedOut
                                | ErrorCode::Cancelled
                                | ErrorCode::DiagnosticChannel
                        ) =>
                    {
                        return Err(error);
                    }
                    Err(error) => {
                        sink.error(&error, control)?;
                        report.errors += 1;
                    }
                }
                Ok(())
            })?;
        }
        if schema >= 6 {
            let head = self.current_knowledge()?;
            if head.is_some() || self.legacy_manifest(control)?.is_some() {
                let _metadata = memory.reserve(2 * 1024 * 1024, control.position())?;
                self.visit_legacy(control, &mut |_, _| Ok(()))?;
                let mut parent = None;
                self.knowledge_history(head.as_ref(), control, &mut |id, event, _| {
                    if event.change.expected_base != parent {
                        return Err(integrity("knowledge chain is not contiguous"));
                    }
                    parent = Some(id.clone());
                    Ok(())
                })?;
            }
        }
        if schema >= 2 {
            let mut statement = connection
                .prepare("SELECT sequence FROM runs ORDER BY sequence")
                .map_err(db)?;
            let mut rows = statement.query([]).map_err(db)?;
            while let Some(row) = rows.next().map_err(db)? {
                control.checkpoint(1)?;
                let sequence: i64 = row.get(0).map_err(db)?;
                // Incremental SQLite access obtains the size without materializing
                // an input-controlled TEXT cell inside SQLite before admission.
                let blob = connection
                    .blob_open(rusqlite::MAIN_DB, "runs", "record", sequence, true)
                    .map_err(db)?;
                let length = blob.len();
                let _record = memory.reserve(
                    (length as u64)
                        .checked_mul(64)
                        .and_then(|n| n.checked_add(4096))
                        .ok_or_else(|| integrity("run record size overflow"))?,
                    control.position(),
                )?;
                let mut raw = memory.bytes(length, control.position())?;
                for (index, chunk) in raw.chunks_mut(WORK_BLOCK).enumerate() {
                    control.bytes(chunk.len())?;
                    blob.read_at_exact(chunk, index * WORK_BLOCK).map_err(db)?;
                }
                let raw =
                    std::str::from_utf8(&raw).map_err(|_| integrity("run record is not UTF-8"))?;
                let run = super::jobs::decode_run(raw)?;
                let identity = connection
                    .blob_open(rusqlite::MAIN_DB, "runs", "id", sequence, true)
                    .map_err(db)?;
                if identity.len() != 64 {
                    return Err(integrity("run row identity length mismatch"));
                }
                let mut id = [0; 64];
                identity.read_at_exact(&mut id, 0).map_err(db)?;
                if id != run.id.as_str().as_bytes() {
                    return Err(integrity("run row identity mismatch"));
                }
                if run.execution.is_some() {
                    let result = (|| {
                        let lease = self.execution_from_run(&run, memory, control)?;
                        validate_execution_records(&lease.manifest, &lease.records, control)
                    })();
                    if let Err(error) = result {
                        if matches!(
                            error.code,
                            ErrorCode::ResourceLimited
                                | ErrorCode::Cancelled
                                | ErrorCode::TimedOut
                                | ErrorCode::DiagnosticChannel
                        ) {
                            return Err(error);
                        }
                        sink.error(&error, control)?;
                        report.errors += 1;
                    }
                }
                if !run.state.terminal() {
                    sink.unfinished(&run.id, control)?;
                    report.unfinished_runs += 1;
                }
            }
        }
        Ok(report)
    }
}
