//! Trial links propose ROM companions; they never select or publish an image.
use super::*;

/// A captured linked image read through its temporary file.
struct TrialImage(std::fs::File);
impl ByteSource for TrialImage {
    fn len(&self) -> u64 {
        self.0.metadata().map_or(0, |m| m.len())
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8], c: &mut dyn RunControl) -> Result<()> {
        use std::os::unix::fs::FileExt;
        c.bytes(bytes.len())?;
        self.0.read_exact_at(bytes, offset).map_err(storage_io)
    }
}

/// Defined functions and data objects of the candidate inputs, by name.
struct Definitions<'a> {
    candidates: &'a [u64],
    names: &'a [String],
    input: u64,
    executable: bool,
    /// (candidate position, name index, local binding, selection)
    found: Vec<(usize, usize, bool, EntrySelection)>,
}
impl InventorySink for Definitions<'_> {
    fn input(&mut self, i: u64, _: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.input = i;
        Ok(())
    }
    fn object(&mut self, o: &ObjectInventory, _: &mut dyn RunControl) -> Result<()> {
        self.executable = o.id.location == ObjectLocation::Standalone
            && o.elf.as_ref().is_some_and(|e| {
                e.object_type == 2 && e.bits == 32 && e.machine == 243 && e.little_endian
            });
        Ok(())
    }
}
impl ElfSink for Definitions<'_> {
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        let Some(position) = self.candidates.iter().position(|i| *i == self.input) else {
            return Ok(());
        };
        // STT_OBJECT or STT_FUNC in a static table.
        if !self.executable
            || !matches!(r.symbol_type, 1 | 2)
            || r.raw_section == 0
            || r.id.table != SymbolTableKind::Static
        {
            return Ok(());
        }
        let Some(name) = r.name.as_deref() else {
            return Ok(());
        };
        if let Some(index) = self.names.iter().position(|n| n.as_bytes() == name) {
            self.found.push((
                position,
                index,
                r.binding == 0,
                EntrySelection {
                    input: self.input,
                    symbol: r.id.clone(),
                },
            ));
        }
        Ok(())
    }
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}

/// Link `request` with unresolved names permitted, then resolve each name the
/// closure leaves undefined against `candidates` in their explicit order.
/// A name resolves to the first candidate input that defines it. Within that
/// input a single global or weak definition is taken; a local one only when no
/// global or weak one exists. Several definitions of the chosen binding, or
/// none in any input, leave the name unresolved.
#[allow(clippy::too_many_arguments)]
pub(crate) fn propose_companions(
    project: &Path,
    request: &LinkRequest,
    candidates: &[u64],
    executable: &Path,
    host: &dyn LinkerHost,
    workspace: &LinkWorkspace<'_>,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<CompanionProposal> {
    if candidates.is_empty()
        || candidates.len() > 16
        || candidates.iter().any(|c| request.inputs.contains(c))
        || candidates
            .iter()
            .enumerate()
            .any(|(i, c)| candidates[..i].contains(c))
    {
        return Err(invalid(
            "companion candidates must be 1-16 distinct inputs outside the link inputs",
        ));
    }
    let _fixed = memory.reserve(LINK_METADATA_BYTES, control.position())?;
    let identity = host.identify(executable, workspace, control)?;
    let project = Project::open(project)?;
    let directory = workspace.directory().to_path_buf();
    control.phase(RunPhase::Materialize)?;
    let found = collect(
        &project,
        request,
        memory,
        control,
        |member, source, _, control| {
            let mut file = workspace.disk.create(&directory.join(&member.alias))?;
            crate::query_stream::copy_source(source, &mut file, control)?;
            file.sync_all().map_err(storage_io)
        },
    )?;
    if !found.blockers.is_empty() {
        return Err(Error::new(
            ErrorCode::LinkBlocked,
            found.blockers[0].message.clone(),
        ));
    }
    let mut forced = Vec::new();
    for root in &found.roots {
        let member = found
            .members
            .iter()
            .find(|m| m.input == root.selection.input && m.object == root.selection.symbol.object)
            .unwrap();
        if !forced.contains(&member.alias) {
            forced.push(member.alias.clone());
        }
    }
    let mut inputs = Vec::new();
    for input in &request.inputs {
        let members: Vec<_> = found
            .members
            .iter()
            .filter(|m| m.input == *input && !forced.contains(&m.alias))
            .collect();
        if members.is_empty() {
            continue;
        }
        if members[0].object.location == ObjectLocation::Standalone {
            inputs.push(LinkInput::Object(members[0].alias.clone()));
        } else {
            inputs.push(LinkInput::Archive(
                members.iter().map(|m| m.alias.clone()).collect(),
            ));
        }
    }
    let definitions = crate::companions::resolve(&project, request, memory, control)?;
    let mut outputs = Outputs::new(workspace)?;
    control.phase(RunPhase::Link)?;
    let invocation = LinkInvocation {
        executable,
        identity: &identity,
        workspace,
        contract: LinkerContract::ElfAnalysisLinkV1,
        entry: &found.roots[0].name,
        roots: found.roots.iter().skip(1).map(|r| r.name.clone()).collect(),
        forced,
        inputs,
        members: found
            .members
            .iter()
            .map(|m| LinkMember {
                alias: m.alias.clone(),
                occurrence: LinkObject {
                    input: m.input,
                    object: m.object.clone(),
                },
            })
            .collect(),
        layout: request.layout,
        definitions,
        unresolved: UnresolvedSymbols::Report,
    };
    if let Err(mut error) = host.link(&invocation, &mut outputs, control) {
        if error.code == ErrorCode::LinkFailed {
            error.message = format!(
                "{}: {}",
                error.message,
                String::from_utf8_lossy(&outputs.stderr)
            );
            truncate_message(&mut error.message);
        }
        return Err(error);
    }
    let image = TrialImage(std::fs::File::open(outputs.elf.path()).map_err(storage_io)?);
    let names = blobray_artifacts::undefined_names(&image, memory, control)?;
    let mut scan = Definitions {
        candidates,
        names: &names,
        input: 0,
        executable: false,
        found: Vec::new(),
    };
    project.read_inventory(request.revision.as_ref(), memory, control, &mut scan)?;
    let mut proposal = CompanionProposal {
        resolved: Vec::new(),
        unresolved: Vec::new(),
    };
    for (index, name) in names.iter().enumerate() {
        control.checkpoint(1)?;
        let first = scan
            .found
            .iter()
            .filter(|(_, i, _, _)| *i == index)
            .map(|(position, _, _, _)| *position)
            .min();
        let in_first: Vec<_> = scan
            .found
            .iter()
            .filter(|(position, i, _, _)| *i == index && Some(*position) == first)
            .collect();
        let local = in_first.iter().all(|(_, _, local, _)| *local);
        let selected: Vec<_> = in_first
            .into_iter()
            .filter(|(_, _, is_local, _)| *is_local == local)
            .collect();
        match selected.as_slice() {
            [(_, _, _, selection)] => proposal.resolved.push(ProposedCompanion {
                name: name.clone(),
                selection: selection.clone(),
            }),
            _ => proposal.unresolved.push(name.clone()),
        }
    }
    Ok(proposal)
}
