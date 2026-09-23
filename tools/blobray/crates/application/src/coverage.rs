//! Streaming join of publication selection and captured inventory. Only one
//! object's extents/sections are resident; this never discovers new functions.
use crate::*;
use blobray_store::JsonlCursor;

type Emit<'a> = dyn FnMut(&ExtentCoverageRecord, &mut dyn RunControl) -> Result<()> + 'a;
struct Selected<'a> {
    symbol: SymbolId,
    extent: CodeRange,
    space: CodeAddressSpace,
    seen: bool,
    _capacity: MemoryReservation<'a>,
}
struct Scan<'a> {
    memory: &'a WorkingMemory,
    cursor: JsonlCursor<'a>,
    pending: Option<InvestigationMember>,
    emit: &'a mut Emit<'a>,
    source: FunctionSource,
    input: u64,
    object: Option<ObjectId>,
    selected: AdmittedVec<'a, Selected<'a>>,
    sections_sorted: bool,
    sections: AdmittedVec<'a, (u32, u64, u64, bool)>,
    ranges: AdmittedVec<'a, (u32, u64, u64)>,
    summary: ExtentCoverageSummary,
}
impl Scan<'_> {
    fn unknown(&mut self, reason: impl Into<String>, c: &mut dyn RunControl) -> Result<()> {
        self.summary.unknowns += 1;
        (self.emit)(
            &ExtentCoverageRecord::Unknown {
                source: self.object.as_ref().map(|_| self.source.clone()),
                object: self.object.clone(),
                reason: reason.into(),
            },
            c,
        )
    }
    fn gap(&mut self, entry: PlanEntry, c: &mut dyn RunControl) -> Result<()> {
        let (source, object, reason) = match entry {
            PlanEntry::Gap {
                input,
                object,
                reason,
            } => (Some(FunctionSource::Input { input }), object, reason),
            PlanEntry::ImageGap { image, reason } => {
                (Some(FunctionSource::Image { image }), None, reason)
            }
            _ => unreachable!(),
        };
        self.summary.unknowns += 1;
        (self.emit)(
            &ExtentCoverageRecord::Unknown {
                source,
                object,
                reason,
            },
            c,
        )
    }
    fn next_group(&mut self, c: &mut dyn RunControl) -> Result<()> {
        loop {
            self.pending = self.cursor.next(c)?;
            match self.pending.as_ref().map(|m| &m.entry) {
                Some(PlanEntry::Input { .. }) => (),
                Some(PlanEntry::Gap { .. } | PlanEntry::ImageGap { .. }) => {
                    self.gap(self.pending.as_ref().unwrap().entry.clone(), c)?;
                }
                Some(PlanEntry::Function { .. }) => {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "publication function outside object group",
                    ));
                }
                _ => return Ok(()),
            }
        }
    }
    fn select(&mut self, object: &ObjectId, c: &mut dyn RunControl) -> Result<()> {
        let matches = match self.pending.as_ref().map(|m| &m.entry) {
            Some(PlanEntry::Object {
                input, object: id, ..
            }) => *input == self.input && id == object,
            Some(PlanEntry::Image { payload, .. }) => *payload == object.artifact,
            _ => false,
        };
        if !matches {
            return Ok(());
        }
        self.object = Some(object.clone());
        self.summary.objects += 1;
        match &self.pending.as_ref().unwrap().entry {
            PlanEntry::Object {
                input, supported, ..
            } => {
                self.source = FunctionSource::Input { input: *input };
                if !supported {
                    self.unknown(
                        format!("unsupported object {} in input {}", object.artifact, input),
                        c,
                    )?;
                }
            }
            PlanEntry::Image { image, .. } => {
                self.source = FunctionSource::Image {
                    image: image.clone(),
                }
            }
            _ => unreachable!(),
        }
        loop {
            self.pending = self.cursor.next(c)?;
            let Some(member) = self.pending.as_ref() else {
                break;
            };
            match &member.entry {
                PlanEntry::Function {
                    request,
                    declared_extent,
                    address_space,
                    ..
                } => {
                    let charge = self.memory.reserve(
                        request.symbol.object.artifact.allocated_bytes(),
                        c.position(),
                    )?;
                    self.selected.push(
                        Selected {
                            symbol: request.symbol.clone(),
                            extent: *declared_extent,
                            space: *address_space,
                            seen: false,
                            _capacity: charge,
                        },
                        c.position(),
                    )?;
                }
                PlanEntry::Gap { .. } | PlanEntry::ImageGap { .. } => {
                    self.gap(self.pending.as_ref().unwrap().entry.clone(), c)?;
                }
                PlanEntry::Input { .. } => (),
                _ => break,
            }
        }
        c.checkpoint(self.selected.len() as u64 * (self.selected.len().max(1).ilog2() as u64 + 1))?;
        self.selected
            .sort_unstable_by(|a, b| a.symbol.cmp(&b.symbol));
        Ok(())
    }
    fn interval(
        &mut self,
        section: u32,
        start: u64,
        end: u64,
        selected: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if start == end {
            return Ok(());
        }
        let length = end - start;
        let count = if selected {
            &mut self.summary.selected_extent_bytes
        } else {
            &mut self.summary.outside_selected_extent_bytes
        };
        *count = count
            .checked_add(length)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "coverage size overflow"))?;
        (self.emit)(
            &ExtentCoverageRecord::Interval {
                source: self.source.clone(),
                object: self.object.as_ref().unwrap().clone(),
                section,
                range: CodeRange { start, length },
                classification: if selected {
                    ExtentClassification::SelectedExtent
                } else {
                    ExtentClassification::OutsideSelectedExtents
                },
            },
            c,
        )
    }
    fn finish(&mut self, c: &mut dyn RunControl) -> Result<()> {
        if self.object.is_none() {
            return Ok(());
        }
        if self.selected.iter().any(|s| !s.seen) {
            self.unknown(
                "selected symbols missing from captured section inventory",
                c,
            )?;
        }
        c.checkpoint(self.ranges.len() as u64 * (self.ranges.len().max(1).ilog2() as u64 + 1))?;
        self.ranges.sort_unstable();
        for i in 0..self.sections.len() {
            c.checkpoint(1)?;
            let (section, address, size, file_backed) = self.sections[i];
            self.summary.executable_bytes = self
                .summary
                .executable_bytes
                .checked_add(size)
                .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "coverage size overflow"))?;
            (self.emit)(
                &ExtentCoverageRecord::Section {
                    source: self.source.clone(),
                    object: self.object.as_ref().unwrap().clone(),
                    section,
                    address,
                    size,
                    file_backed,
                },
                c,
            )?;
            let mut cursor = 0;
            let mut index = self.ranges.partition_point(|r| r.0 < section);
            while index < self.ranges.len() && self.ranges[index].0 == section {
                c.checkpoint(1)?;
                let (_, start, mut end) = self.ranges[index];
                index += 1;
                while index < self.ranges.len()
                    && self.ranges[index].0 == section
                    && self.ranges[index].1 <= end
                {
                    c.checkpoint(1)?;
                    end = end.max(self.ranges[index].2);
                    index += 1;
                }
                self.interval(section, cursor, start, false, c)?;
                self.interval(section, start, end, true, c)?;
                cursor = end;
            }
            self.interval(section, cursor, size, false, c)?;
        }
        self.object = None;
        self.selected = AdmittedVec::new(self.memory);
        self.sections = AdmittedVec::new(self.memory);
        self.sections_sorted = false;
        self.ranges = AdmittedVec::new(self.memory);
        Ok(())
    }
}
impl InventorySink for Scan<'_> {
    fn input(&mut self, n: u64, _: &InputRecord, c: &mut dyn RunControl) -> Result<()> {
        self.finish(c)?;
        self.input = n;
        Ok(())
    }
    fn object(&mut self, object: &ObjectInventory, c: &mut dyn RunControl) -> Result<()> {
        self.finish(c)?;
        self.select(&object.id, c)
    }
}
impl ElfSink for Scan<'_> {
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn section(&mut self, s: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        if self.object.is_some() && s.flags & 4 != 0 && s.size != 0 {
            self.sections_sorted = false;
            self.sections.push(
                (s.index, s.address, s.size, s.section_type != 8),
                c.position(),
            )?;
        }
        Ok(())
    }
    fn symbol(&mut self, s: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        if self.object.is_none() {
            return Ok(());
        }
        c.checkpoint(self.selected.len().max(1).ilog2() as u64 + 1)?;
        if !self.sections_sorted {
            c.checkpoint(
                self.sections.len() as u64 * (self.sections.len().max(1).ilog2() as u64 + 1),
            )?;
            self.sections.sort_unstable_by_key(|s| s.0);
            self.sections_sorted = true;
        }
        let first = self.selected.partition_point(|r| r.symbol < s.id);
        for i in first..self.selected.len() {
            if self.selected[i].symbol != s.id {
                break;
            }
            self.selected[i].seen = true;
            let section = s.extended_section.unwrap_or(u32::from(s.raw_section));
            c.checkpoint(self.sections.len().max(1).ilog2() as u64 + 1)?;
            let Some(&(_, address, size, _)) = self
                .sections
                .binary_search_by_key(&section, |s| s.0)
                .ok()
                .map(|i| &self.sections[i])
            else {
                self.unknown("selected function has no executable section", c)?;
                continue;
            };
            let selected = &self.selected[i];
            let base = if selected.space == CodeAddressSpace::Image {
                address
            } else {
                0
            };
            let range = selected.extent.start.checked_sub(base).and_then(|start| {
                selected
                    .extent
                    .length
                    .checked_add(start)
                    .map(|end| (start, end))
            });
            if let Some((start, end)) = range.filter(|(a, b)| a < b && *b <= size) {
                self.ranges.push((section, start, end), c.position())?;
            } else {
                self.unknown(
                    "selected extent is empty or outside its executable section",
                    c,
                )?;
            }
        }
        Ok(())
    }
    fn diagnostic(&mut self, d: &Diagnostic, c: &mut dyn RunControl) -> Result<()> {
        if self.object.is_some() {
            self.unknown(format!("{}: {}", d.context, d.message), c)?;
        }
        Ok(())
    }
}
pub(crate) fn report(
    project: &Project,
    id: &PublicationId,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut Emit<'_>,
) -> Result<(InvestigationCoverage, ExtentCoverageSummary)> {
    let _metadata = memory.reserve(4 * 1024 * 1024, c.position())?;
    let publication = project.publication(id, c)?;
    let request = &publication.manifest.plan.recipe.request;
    let mut scan = Scan {
        memory,
        cursor: JsonlCursor::new(&publication.members)?,
        pending: None,
        emit,
        source: FunctionSource::Input { input: 0 },
        input: 0,
        object: None,
        selected: AdmittedVec::new(memory),
        sections_sorted: false,
        sections: AdmittedVec::new(memory),
        ranges: AdmittedVec::new(memory),
        summary: ExtentCoverageSummary::default(),
    };
    scan.next_group(c)?;
    if let Some(id) = &request.image {
        let image = project.image(id, c)?;
        let object = ObjectId {
            artifact: image.manifest.elf,
            location: ObjectLocation::Standalone,
        };
        scan.select(&object, c)?;
        blobray_artifacts::inspect_source(&image.elf, &object, memory, c, &mut scan)?;
    } else {
        project.read_inventory(request.revision.as_ref(), memory, c, &mut scan)?;
    }
    scan.finish(c)?;
    if scan.pending.is_some() {
        return Err(Error::new(
            ErrorCode::Integrity,
            "publication object missing from captured inventory",
        ));
    }
    Ok((publication.manifest.coverage, scan.summary))
}
