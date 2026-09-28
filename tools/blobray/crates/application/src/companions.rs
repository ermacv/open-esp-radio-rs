//! Explicit ROM definitions are captured occurrences, never fabricated implementations.
use crate::*;
struct Scan<'a> {
    request: &'a LinkRequest,
    input: u64,
    executable: bool,
    matches: Vec<Option<SymbolRecord>>,
}
impl InventorySink for Scan<'_> {
    fn input(&mut self, i: u64, _: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.input = i;
        Ok(())
    }
    fn object(&mut self, o: &ObjectInventory, _: &mut dyn RunControl) -> Result<()> {
        self.executable = o.elf.as_ref().is_some_and(|e| {
            e.object_type == 2 && e.bits == 32 && e.machine == 243 && e.little_endian
        });
        Ok(())
    }
}
impl ElfSink for Scan<'_> {
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        for (i, selection) in self.request.companions.iter().enumerate() {
            if selection.input == self.input && selection.symbol == r.id {
                // STT_OBJECT (1) or STT_FUNC (2); the carrier validates the extent.
                if !self.executable
                    || !matches!(r.symbol_type, 1 | 2)
                    || r.raw_section == 0
                    || r.id.object.location != ObjectLocation::Standalone
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "companion requires a defined function or data object in a captured static RV32 ELF",
                    ));
                }
                self.matches[i] = Some(r.clone());
            }
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
/// Fail when a selected link input's symbol `r` defines a companion name.
/// The link's own pass over its inputs applies this, so resolving the
/// companions reads only the companion inputs.
pub(crate) fn check_link_symbol(names: &[(String, u32)], r: &SymbolRecord) -> Result<()> {
    if r.raw_section != 0
        && r.binding != 0
        && r.name
            .as_ref()
            .is_some_and(|n| names.iter().any(|(name, _)| name.as_bytes() == n))
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "companion name also defined by a selected link input",
        ));
    }
    Ok(())
}
/// Linker definitions of the request's companions, in selection order,
/// then of its absent names. A selected link input must not define a
/// companion name; see [`check_link_symbol`].
pub(crate) fn resolve(
    project: &Project,
    request: &LinkRequest,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<(String, u32)>> {
    let absent = request
        .absent
        .iter()
        .map(|name| (name.clone(), ABSENT_SYMBOL_ADDRESS));
    if request.companions.is_empty() {
        return Ok(absent.collect());
    }
    if request.companions.len() > 64 {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "at most 64 exact ROM definitions",
        ));
    }
    let _capacity = memory.reserve(1024 * 1024, c.position())?;
    let mut scan = Scan {
        request,
        input: 0,
        executable: false,
        matches: vec![None; request.companions.len()],
    };
    crate::linking::read_inputs(
        project,
        request.revision.as_ref(),
        request.companions.iter().map(|s| s.input),
        memory,
        c,
        &mut scan,
    )?;
    let mut definitions = Vec::new();
    for (selection, record) in request.companions.iter().zip(&scan.matches) {
        let record = record
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "companion occurrence absent"))?;
        let name = String::from_utf8(record.name.clone().unwrap_or_default())
            .map_err(|_| Error::new(ErrorCode::InvalidRequest, "non-UTF8 companion linker name"))?;
        if name.is_empty()
            || name.len() > 512
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'$'))
            || definitions.iter().any(|(old, _)| old == &name)
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid or duplicate companion name",
            ));
        }
        let source = project.open_payload(&selection.symbol.object.artifact, c)?;
        let address =
            blobray_artifacts::inspect_link_definition(&source, &selection.symbol, memory, c)?;
        if request.layout.code.contains(u64::from(address), 1)
            || request.layout.data.contains(u64::from(address), 1)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "ROM address overlaps synthetic placement",
            ));
        }
        definitions.push((name, address));
    }
    for (name, address) in absent {
        if definitions.iter().any(|(old, _)| *old == name) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "an absent name is also a companion",
            ));
        }
        definitions.push((name, address));
    }
    Ok(definitions)
}
