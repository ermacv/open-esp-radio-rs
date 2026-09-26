//! Name convenience resolves to the same exact link request as API callers.
use crate::*;

struct EntrySearch<'a> {
    request: &'a NamedLinkRequest,
    revision: &'a RevisionId,
    sink: &'a mut dyn QuerySink,
    selected: bool,
    input: u64,
    companions: Vec<Option<EntrySelection>>,
    companion_counts: Vec<u64>,
    count: u64,
    candidate: Option<EntrySelection>,
}
impl InventorySink for EntrySearch<'_> {
    fn input(&mut self, n: u64, _: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.input = n;
        self.selected = n == self.request.entry_input;
        Ok(())
    }
}
impl ElfSink for EntrySearch<'_> {
    fn symbol(&mut self, symbol: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        for (i, wanted) in self.request.companions.iter().enumerate() {
            if wanted.input == self.input
                && symbol.id.table == SymbolTableKind::Static
                && symbol.symbol_type == 2
                && symbol.raw_section != 0
                && symbol.name.as_deref() == Some(wanted.name.as_bytes())
            {
                self.companion_counts[i] += 1;
                self.companions[i] = Some(EntrySelection {
                    input: self.input,
                    symbol: symbol.id.clone(),
                });
            }
        }
        if self.selected
            && symbol.id.table == SymbolTableKind::Static
            && symbol.symbol_type == 2
            && symbol.raw_section != 0
            && symbol.name.as_deref() == Some(&self.request.entry_name)
        {
            self.count += 1;
            self.sink.candidate(
                &SelectionCandidate {
                    revision: self.revision.clone(),
                    scope: InspectionScope::Symbol {
                        input: self.request.entry_input,
                        symbol: symbol.id.clone(),
                    },
                    name: self.request.entry_name.clone(),
                    payload: None,
                },
                c,
            )?;
            if self.count == 1 {
                self.candidate = Some(EntrySelection {
                    input: self.request.entry_input,
                    symbol: symbol.id.clone(),
                });
            } else {
                self.candidate = None;
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
    fn diagnostic(&mut self, r: &Diagnostic, c: &mut dyn RunControl) -> Result<()> {
        self.sink.diagnostic(r, c)
    }
}
pub(crate) fn resolve(
    project: &Path,
    request: &NamedLinkRequest,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    sink: &mut dyn QuerySink,
) -> Result<(Option<LinkRequest>, u64, bool)> {
    let revision = request.revision.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidRequest,
            "link selection revision is not frozen",
        )
    })?;
    if request.entry_name.is_empty()
        || request.entry_name.len() > 4096
        || !request.inputs.contains(&request.entry_input)
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "entry must have a bounded name and belong to the ordered input selection",
        ));
    }
    if request.companions.len() > 64 {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "too many companions",
        ));
    }
    let _capacity = memory.reserve(1024 * 1024, c.position())?;
    let mut search = EntrySearch {
        request,
        revision,
        sink,
        selected: false,
        input: 0,
        companions: vec![None; request.companions.len()],
        companion_counts: vec![0; request.companions.len()],
        count: 0,
        candidate: None,
    };
    let inventory = ReadView::open(project)?.inventory(revision, memory, c, &mut search)?;
    if search.companion_counts.iter().any(|n| *n != 1) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "companion name absent or ambiguous; use exact selectors",
        ));
    }
    Ok((
        search.candidate.map(|entry| LinkRequest {
            companions: search.companions.into_iter().flatten().collect(),
            revision: Some(revision.clone()),
            inputs: request.inputs.clone(),
            entry,
            roots: Vec::new(),
            layout: request.layout,
            absent: vec![],
        }),
        search.count,
        inventory.complete(),
    ))
}
