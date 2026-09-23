//! Streaming selection over the shared manifest reader. No name resolution policy.
use crate::*;

pub(crate) struct Probe<'a> {
    pub scope: &'a InspectionScope,
    pub header: Option<RevisionHeader>,
    pub binding: Option<SelectedBinding>,
    pub found: bool,
    pub section: Option<u32>,
    input: bool,
    object: bool,
}
impl<'a> Probe<'a> {
    pub fn new(scope: &'a InspectionScope) -> Self {
        Self {
            scope,
            header: None,
            binding: None,
            found: matches!(scope, InspectionScope::Revision),
            section: None,
            input: false,
            object: false,
        }
    }
    pub fn recipe(
        &self,
        revision: RevisionId,
        complete: bool,
        budget: ResourceBudget,
    ) -> Result<InspectionRecipe> {
        if !self.found {
            return Err(Error::new(
                ErrorCode::NotFound,
                "selected occurrence is absent from this revision/input",
            ));
        }
        let header = self
            .header
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "manifest header missing"))?;
        Ok(InspectionRecipe {
            schema: 1,
            operation_version: 1,
            result_schema: 1,
            project: header.project.clone(),
            revision,
            scope: self.scope.clone(),
            target: header.target,
            inventory_producer: header.inventory_producer.clone(),
            binding: self.binding.clone(),
            revision_complete: complete,
            budget,
        })
    }
}
fn object_matches(scope: &InspectionScope, object: &ObjectId) -> bool {
    match scope {
        InspectionScope::Object {
            object: expected, ..
        } => expected == object,
        InspectionScope::Symbol { symbol, .. } => &symbol.object == object,
        _ => true,
    }
}
impl InventorySink for Probe<'_> {
    fn revision(&mut self, header: &RevisionHeader, _: &mut dyn RunControl) -> Result<()> {
        // Bound retained recipe metadata before cloning it out of a borrowed callback.
        crate::protocol::write_request(std::io::sink(), header)?;
        self.header = Some(header.clone());
        Ok(())
    }
    fn input(&mut self, n: u64, input: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.input = self.scope.input().is_none_or(|wanted| wanted == n);
        self.object = false;
        if self.input && self.scope.input().is_some() {
            crate::protocol::write_request(std::io::sink(), &input.capture)?;
            self.binding = Some(SelectedBinding {
                input: n,
                capture: input.capture.clone(),
                payload: None,
            });
            if matches!(self.scope, InspectionScope::Input { .. }) {
                self.found = true;
            }
        }
        Ok(())
    }
    fn object(&mut self, object: &ObjectInventory, _: &mut dyn RunControl) -> Result<()> {
        self.object = self.input && object_matches(self.scope, &object.id);
        if self.object
            && matches!(
                self.scope,
                InspectionScope::Object { .. } | InspectionScope::Symbol { .. }
            )
        {
            self.binding.as_mut().unwrap().payload = object.content.clone();
            if matches!(self.scope, InspectionScope::Object { .. }) {
                self.found = true;
            }
        }
        Ok(())
    }
}
impl ElfSink for Probe<'_> {
    fn symbol(&mut self, record: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        if self.object
            && let InspectionScope::Symbol { symbol, .. } = self.scope
            && symbol == &record.id
        {
            self.found = true;
            self.section = if record.raw_section == 0xffff {
                record.extended_section
            } else if record.raw_section > 0 && record.raw_section < 0xff00 {
                Some(record.raw_section as u32)
            } else {
                None
            };
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
pub(crate) struct Filter<'a> {
    scope: &'a InspectionScope,
    section: Option<u32>,
    sink: &'a mut dyn InventorySink,
    input: bool,
    object: bool,
    before_object: bool,
}
impl<'a> Filter<'a> {
    pub fn new(
        scope: &'a InspectionScope,
        section: Option<u32>,
        sink: &'a mut dyn InventorySink,
    ) -> Self {
        Self {
            scope,
            section,
            sink,
            input: false,
            object: false,
            before_object: true,
        }
    }
}
impl InventorySink for Filter<'_> {
    fn revision(&mut self, r: &RevisionHeader, c: &mut dyn RunControl) -> Result<()> {
        self.sink.revision(r, c)
    }
    fn input(&mut self, n: u64, r: &InputRecord, c: &mut dyn RunControl) -> Result<()> {
        self.input = self.scope.input().is_none_or(|wanted| wanted == n);
        self.object = false;
        self.before_object = true;
        if self.input {
            self.sink.input(n, r, c)?;
        }
        Ok(())
    }
    fn object(&mut self, r: &ObjectInventory, c: &mut dyn RunControl) -> Result<()> {
        self.before_object = false;
        self.object = self.input && object_matches(self.scope, &r.id);
        if self.object {
            self.sink.object(r, c)?;
        }
        Ok(())
    }
    fn external(&mut self, r: &ExternalMember, c: &mut dyn RunControl) -> Result<()> {
        let selected = match self.scope {
            InspectionScope::Object { object, .. } => {
                object.location == (ObjectLocation::ArchiveMember { ordinal: r.ordinal })
            }
            InspectionScope::Symbol { symbol, .. } => {
                symbol.object.location == (ObjectLocation::ArchiveMember { ordinal: r.ordinal })
            }
            _ => true,
        };
        if self.input && selected {
            self.sink.external(r, c)?;
        }
        Ok(())
    }
}
impl ElfSink for Filter<'_> {
    fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        if self.object
            && (!matches!(self.scope, InspectionScope::Symbol { .. })
                || self.section == Some(r.index))
        {
            self.sink.section(r, c)?;
        }
        Ok(())
    }
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        if self.object
            && match self.scope {
                InspectionScope::Symbol { symbol, .. } => symbol == &r.id,
                _ => true,
            }
        {
            self.sink.symbol(r, c)?;
        }
        Ok(())
    }
    fn relocation(&mut self, r: &RelocationRecord, c: &mut dyn RunControl) -> Result<()> {
        if self.object
            && match self.scope {
                InspectionScope::Symbol { symbol, .. } => {
                    r.symbol_table_section == symbol.table_section
                        && u64::from(r.symbol_index) == symbol.index
                }
                _ => true,
            }
        {
            self.sink.relocation(r, c)?;
        }
        Ok(())
    }
    fn diagnostic(&mut self, r: &Diagnostic, c: &mut dyn RunControl) -> Result<()> {
        if self.input && (self.before_object || self.object) {
            self.sink.diagnostic(r, c)?;
        }
        Ok(())
    }
}
pub(crate) struct Search<'a> {
    pub request: &'a SelectionRequest,
    pub revision: &'a RevisionId,
    pub sink: &'a mut dyn QuerySink,
    pub count: u64,
    input: u64,
    payload: Option<ArtifactId>,
}
impl<'a> Search<'a> {
    pub fn new(
        request: &'a SelectionRequest,
        revision: &'a RevisionId,
        sink: &'a mut dyn QuerySink,
    ) -> Self {
        Self {
            request,
            revision,
            sink,
            count: 0,
            input: 0,
            payload: None,
        }
    }
    fn selected(&self) -> bool {
        self.request.input.is_none_or(|i| i == self.input)
    }
    fn candidate(&mut self, scope: InspectionScope, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "candidate count overflow"))?;
        self.sink.candidate(
            &SelectionCandidate {
                revision: self.revision.clone(),
                scope,
                name: self.request.name.clone(),
                payload: self.payload.clone(),
            },
            c,
        )
    }
}
impl InventorySink for Search<'_> {
    fn input(&mut self, n: u64, r: &InputRecord, c: &mut dyn RunControl) -> Result<()> {
        self.input = n;
        self.payload = None;
        if self.selected()
            && let Capture::Unavailable { diagnostic } = &r.capture
        {
            self.sink.diagnostic(diagnostic, c)?;
        }
        Ok(())
    }
    fn object(&mut self, r: &ObjectInventory, c: &mut dyn RunControl) -> Result<()> {
        self.payload = r.content.clone();
        if self.selected()
            && self.request.kind == SelectionKind::Object
            && r.name.as_deref() == Some(&self.request.name)
        {
            self.candidate(
                InspectionScope::Object {
                    input: self.input,
                    object: r.id.clone(),
                },
                c,
            )?;
        }
        Ok(())
    }
}
impl ElfSink for Search<'_> {
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        if self.selected()
            && self.request.kind == SelectionKind::Symbol
            && r.name.as_deref() == Some(&self.request.name)
        {
            self.candidate(
                InspectionScope::Symbol {
                    input: self.input,
                    symbol: r.id.clone(),
                },
                c,
            )?;
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
        if self.selected() {
            self.sink.diagnostic(r, c)?;
        }
        Ok(())
    }
}
