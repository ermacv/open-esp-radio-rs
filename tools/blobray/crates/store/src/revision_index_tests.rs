//! Scoped revision reads through the object index agree with the full walk.
use super::*;

/// Counts work units so a scoped read can be compared with a full walk.
struct Units(u64);
impl RunControl for Units {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.0 += units.max(1);
        Ok(())
    }
}

/// Every callback as text, restricted to one input and, when given, one object.
struct Recorder {
    input: u64,
    object: Option<ObjectId>,
    selected_input: bool,
    selected_object: bool,
    lines: Vec<String>,
}
impl Recorder {
    fn new(input: u64, object: Option<ObjectId>) -> Self {
        Self {
            input,
            object,
            selected_input: false,
            selected_object: false,
            lines: vec![],
        }
    }
    fn push(&mut self, line: String) {
        if self.selected_object {
            self.lines.push(line);
        }
    }
}
impl ElfSink for Recorder {
    fn section(&mut self, record: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        self.push(format!("section {record:?}"));
        Ok(())
    }
    fn symbol(&mut self, record: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        self.push(format!("symbol {record:?}"));
        Ok(())
    }
    fn relocation(&mut self, record: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        self.push(format!("relocation {record:?}"));
        Ok(())
    }
    fn diagnostic(&mut self, record: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        if self.selected_input {
            self.lines.push(format!("diagnostic {record:?}"));
        }
        Ok(())
    }
}
impl InventorySink for Recorder {
    fn revision(&mut self, header: &RevisionHeader, _: &mut dyn RunControl) -> Result<()> {
        self.lines.push(format!("revision {header:?}"));
        Ok(())
    }
    fn input(&mut self, n: u64, input: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.selected_input = n == self.input;
        self.selected_object = false;
        if self.selected_input {
            self.lines.push(format!("input {n} {input:?}"));
        }
        Ok(())
    }
    fn container(
        &mut self,
        kind: ContainerKind,
        complete: bool,
        _: &mut dyn RunControl,
    ) -> Result<()> {
        if self.selected_input {
            self.lines.push(format!("container {kind:?} {complete}"));
        }
        Ok(())
    }
    fn object(&mut self, object: &ObjectInventory, _: &mut dyn RunControl) -> Result<()> {
        self.selected_object =
            self.selected_input && self.object.as_ref().is_none_or(|o| *o == object.id);
        self.push(format!("object {object:?}"));
        Ok(())
    }
}

fn owner() -> OwnerIdentity {
    OwnerIdentity {
        pid: 123,
        start_ticks: 42,
        boot_id: "test".into(),
    }
}

fn elf(object: &ObjectId, symbols: u64) -> ElfInventory {
    ElfInventory {
        bits: 32,
        little_endian: true,
        machine: 243,
        object_type: 1,
        entry: 0,
        sections: vec![SectionRecord {
            index: 1,
            name: Some(b".text".to_vec()),
            section_type: 1,
            flags: 6,
            address: 0,
            file_offset: 52,
            size: 8,
            link: 0,
            info: 0,
            alignment: 4,
            entry_size: 0,
        }],
        symbols: (1..=symbols)
            .map(|index| SymbolRecord {
                id: SymbolId {
                    object: object.clone(),
                    table: SymbolTableKind::Static,
                    table_section: 2,
                    index,
                },
                name: Some(format!("f{index}").into_bytes()),
                name_offset: index as u32,
                value: index * 4,
                size: 4,
                binding: 1,
                symbol_type: 2,
                other: 0,
                raw_section: 1,
                extended_section: None,
            })
            .collect(),
        relocations: vec![],
    }
}

const MEMBERS: u64 = 40;

/// A published revision with an archive of many members and a standalone ELF.
fn fixture(temp: &Path) -> (Project, RevisionId, Vec<ArtifactId>) {
    let project = Project::create(temp).unwrap();
    let mut writer = project.writer().unwrap();
    let (mut run, stage) = writer.register(ResourceBudget::default(), owner()).unwrap();
    let mut staging = Staging::open(&stage).unwrap();
    let archive_bytes = b"!<arch>\nfixture archive".to_vec();
    let standalone_bytes = b"\x7fELF fixture".to_vec();
    let archive = staging
        .retain_bytes(&archive_bytes, &mut || Ok(()))
        .unwrap();
    let standalone = staging
        .retain_bytes(&standalone_bytes, &mut || Ok(()))
        .unwrap();
    let captured = |artifact: &ArtifactId, length: usize| Capture::Captured {
        artifact: artifact.clone(),
        length: length as u64,
    };
    let members = (0..MEMBERS)
        .map(|ordinal| {
            let id = ObjectId {
                artifact: archive.clone(),
                location: ObjectLocation::ArchiveMember { ordinal },
            };
            ObjectInventory {
                elf: Some(elf(&id, 8)),
                id,
                name: Some(format!("m{ordinal}.o").into_bytes()),
                content: Some(ArtifactId::of_bytes(format!("member {ordinal}").as_bytes())),
                diagnostics: vec![],
            }
        })
        .collect();
    let single = ObjectId {
        artifact: standalone.clone(),
        location: ObjectLocation::Standalone,
    };
    let input = |role: &str, capture, inventory| InputRecord {
        role: role.into(),
        origin: OriginPath::UnixBytes {
            bytes: format!("/{role}").into_bytes(),
        },
        expected: None,
        capture,
        external_members: vec![],
        inventory: Some(inventory),
    };
    let revision = Revision {
        schema: 1,
        project: project.id().clone(),
        parent: None,
        target: Target::Riscv32Ilp32,
        inventory_producer: "test/1".into(),
        inputs: vec![
            input(
                "library",
                captured(&archive, archive_bytes.len()),
                ArtifactInventory {
                    kind: ContainerKind::Archive,
                    members_complete: true,
                    objects: members,
                    diagnostics: vec![],
                },
            ),
            input(
                "production",
                captured(&standalone, standalone_bytes.len()),
                ArtifactInventory {
                    kind: ContainerKind::Elf,
                    members_complete: true,
                    objects: vec![ObjectInventory {
                        elf: Some(elf(&single, 3)),
                        id: single.clone(),
                        name: None,
                        content: Some(standalone.clone()),
                        diagnostics: vec![],
                    }],
                    diagnostics: vec![],
                },
            ),
        ],
    };
    let prepared = staging.prepare(revision).unwrap();
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let retained = writer
        .retain_candidate(&run, &prepared, &memory, &mut || Ok(()))
        .unwrap();
    writer.publish_run(&mut run, retained).unwrap();
    let revision = project.current().unwrap().unwrap();
    (project, revision, vec![archive, standalone])
}

fn full(
    project: &Project,
    revision: &RevisionId,
    input: u64,
    object: Option<&ObjectId>,
) -> (Vec<String>, u64) {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let mut recorder = Recorder::new(input, object.cloned());
    let mut units = Units(0);
    project
        .read_inventory(Some(revision), &memory, &mut units, &mut recorder)
        .unwrap();
    (recorder.lines, units.0)
}

fn scoped(
    project: &Project,
    revision: &RevisionId,
    input: u64,
    object: Option<&ObjectId>,
) -> Result<(Vec<String>, u64)> {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let mut recorder = Recorder::new(input, object.cloned());
    let mut units = Units(0);
    project.read_scoped(revision, input, object, &memory, &mut units, &mut recorder)?;
    Ok((recorder.lines, units.0))
}

#[test]
fn scoped_reads_present_the_full_walk_records_at_a_fraction_of_its_work() {
    let temp = tempfile::tempdir().unwrap();
    let (project, revision, artifacts) = fixture(temp.path());
    let member = ObjectId {
        artifact: artifacts[0].clone(),
        location: ObjectLocation::ArchiveMember { ordinal: 17 },
    };
    let single = ObjectId {
        artifact: artifacts[1].clone(),
        location: ObjectLocation::Standalone,
    };
    for (input, object) in [(0, Some(&member)), (1, Some(&single)), (0, None), (1, None)] {
        let (expected, whole) = full(&project, &revision, input, object);
        let (actual, part) = scoped(&project, &revision, input, object).unwrap();
        assert_eq!(actual, expected, "{input} {object:?}");
        assert!(expected.iter().any(|l| l.starts_with("object")));
        if object.is_some() {
            assert!(part * 4 < whole, "{part} of {whole}");
        }
    }
    // An object of another input or outside the archive yields no records.
    let absent = ObjectId {
        artifact: artifacts[0].clone(),
        location: ObjectLocation::ArchiveMember { ordinal: MEMBERS },
    };
    for (input, object) in [(1, &member), (0, &absent)] {
        let (lines, _) = scoped(&project, &revision, input, Some(object)).unwrap();
        assert!(!lines.iter().any(|l| l.starts_with("object")));
    }
}

#[test]
fn forged_spans_fail_closed_and_a_missing_index_falls_back_to_the_full_walk() {
    let temp = tempfile::tempdir().unwrap();
    let (project, revision, artifacts) = fixture(temp.path());
    let member = ObjectId {
        artifact: artifacts[0].clone(),
        location: ObjectLocation::ArchiveMember { ordinal: 3 },
    };
    let connection = open_connection(&project.root, true).unwrap();
    let forge = |sql: &str| {
        connection.execute(sql, [revision.as_str()]).unwrap();
    };
    // A span that is not one value, and a span of another member.
    forge("UPDATE revision_objects SET start=start+1 WHERE revision=?1 AND input=0 AND ordinal=3");
    assert_eq!(
        scoped(&project, &revision, 0, Some(&member))
            .unwrap_err()
            .code,
        ErrorCode::Integrity
    );
    forge("UPDATE revision_objects SET start=start-1 WHERE revision=?1 AND input=0 AND ordinal=3");
    forge(
        "UPDATE revision_objects SET (start,end)=(SELECT start,end FROM revision_objects o \
         WHERE o.revision=?1 AND o.input=0 AND o.ordinal=4) WHERE revision=?1 AND input=0 AND ordinal=3",
    );
    assert_eq!(
        scoped(&project, &revision, 0, Some(&member))
            .unwrap_err()
            .code,
        ErrorCode::Integrity
    );
    forge("DELETE FROM revision_scopes WHERE revision=?1");
    let (expected, _) = full(&project, &revision, 0, Some(&member));
    assert_eq!(
        scoped(&project, &revision, 0, Some(&member)).unwrap().0,
        expected
    );
}
