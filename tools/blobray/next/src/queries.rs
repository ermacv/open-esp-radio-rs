//! Frontend rendering of a completed, application-owned query result.

use blobray_application as app;
use blobray_domain::*;
use std::io::{BufWriter, Write};

fn io(error: std::io::Error) -> Error {
    storage_io(error)
}
struct Output<'a> {
    file: &'a mut dyn Write,
    control: &'a mut dyn RunControl,
    failure: &'a mut Option<Error>,
}
impl Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let size = bytes.len().min(WORK_BLOCK);
        if let Err(error) = self.control.bytes(size) {
            *self.failure = Some(error.clone());
            return Err(std::io::Error::other(error));
        }
        self.file.write(&bytes[..size]).map_err(|error| {
            let error = storage_io(error);
            *self.failure = Some(error.clone());
            std::io::Error::other(error)
        })
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush().map_err(|error| {
            let error = storage_io(error);
            *self.failure = Some(error.clone());
            std::io::Error::other(error)
        })
    }
}
fn output(
    file: &mut dyn Write,
    control: &mut dyn RunControl,
    f: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> Result<()> {
    let mut failure = None;
    let result = {
        let writer = Output {
            file,
            control,
            failure: &mut failure,
        };
        let mut buffered = BufWriter::with_capacity(WORK_BLOCK, writer);
        f(&mut buffered).and_then(|()| buffered.flush())
    };
    if let Some(error) = failure {
        return Err(error);
    }
    result.map_err(io)
}
fn json<T: serde::Serialize>(
    file: &mut dyn Write,
    value: &T,
    control: &mut dyn RunControl,
) -> Result<()> {
    output(file, control, |writer| {
        serde_json::to_writer(writer, value).map_err(std::io::Error::other)
    })
}
fn raw(file: &mut dyn Write, bytes: &[u8], control: &mut dyn RunControl) -> Result<()> {
    output(file, control, |writer| writer.write_all(bytes))
}

pub(super) fn render(
    result: &mut app::QueryOutput,
    format: super::Format,
    writer: &mut dyn Write,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    if matches!(
        result.summary(),
        app::QuerySummary::Trace { .. }
            | app::QuerySummary::SemanticIr { .. }
            | app::QuerySummary::Registers { .. }
            | app::QuerySummary::EventRoute { .. }
            | app::QuerySummary::MemorySlice { .. }
            | app::QuerySummary::Flow { .. }
            | app::QuerySummary::Navigation { .. }
            | app::QuerySummary::Interfaces { .. }
            | app::QuerySummary::Coverage { .. }
            | app::QuerySummary::StorageUsage { .. }
            | app::QuerySummary::Data { .. }
            | app::QuerySummary::TargetAudit { .. }
            | app::QuerySummary::KnowledgeValidation { .. }
            | app::QuerySummary::RetainedPayload { .. }
            | app::QuerySummary::Legacy { .. }
            | app::QuerySummary::Preservation { .. }
            | app::QuerySummary::Knowledge { .. }
            | app::QuerySummary::InvestigationPlan { .. }
            | app::QuerySummary::Publications { .. }
            | app::QuerySummary::Publication { .. }
            | app::QuerySummary::InvestigationStatus { .. }
            | app::QuerySummary::Selection { .. }
            | app::QuerySummary::Inspection { .. }
            | app::QuerySummary::Analyses { .. }
            | app::QuerySummary::Analysis { .. }
            | app::QuerySummary::Execution { .. }
            | app::QuerySummary::Images { .. }
            | app::QuerySummary::Image { .. }
            | app::QuerySummary::LinkPlan { .. }
    ) {
        return result.records(
            cancelled,
            &mut Records {
                file: writer,
                format,
                count: 0,
                started: false,
            },
        );
    }
    if matches!(result.summary(), app::QuerySummary::Inventory { .. }) {
        if matches!(format, super::Format::Json) {
            return result.manifest(cancelled, |summary, source, control| {
                let app::QuerySummary::Inventory {
                    revision_id,
                    complete,
                } = summary
                else {
                    unreachable!()
                };
                raw(writer, b"{\"schema\":2,\"assessment\":", control)?;
                json(writer, &summary.assessment(), control)?;
                raw(writer, b",\"complete\":", control)?;
                json(writer, complete, control)?;
                raw(writer, b",\"snapshot\":{\"revision_id\":", control)?;
                json(writer, revision_id, control)?;
                raw(writer, b",\"revision\":", control)?;
                let mut bytes = [0; WORK_BLOCK];
                let mut offset = 0;
                while offset < source.len() {
                    let count = (source.len() - offset).min(WORK_BLOCK as u64) as usize;
                    source.read_at(offset, &mut bytes[..count], control)?;
                    raw(writer, &bytes[..count], control)?;
                    offset += count as u64;
                }
                raw(writer, b"}}\n", control)
            });
        }
        result.records(cancelled, &mut Human { file: writer })
    } else {
        result.records(
            cancelled,
            &mut Doctor {
                file: writer,
                format,
                errors: 0,
                unfinished: 0,
                switched: false,
                started: false,
            },
        )
    }
}
struct Human<'a> {
    file: &'a mut dyn Write,
}
impl InventorySink for Human<'_> {
    fn input(
        &mut self,
        ordinal: u64,
        input: &InputRecord,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        output(self.file, control, |writer| {
            writeln!(writer, "Input {ordinal}: {}", input.role)
        })?;
        if let Capture::Unavailable { diagnostic } = &input.capture {
            self.diagnostic(diagnostic, control)?;
        }
        Ok(())
    }
    fn object(&mut self, object: &ObjectInventory, control: &mut dyn RunControl) -> Result<()> {
        output(self.file, control, |writer| {
            writeln!(
                writer,
                "  {:?}: {:?}",
                object.id.location,
                object.name.as_deref().map(String::from_utf8_lossy)
            )
        })?;
        if let Some(elf) = &object.elf {
            output(self.file, control, |writer| {
                writeln!(writer, "    ELF{} machine={}", elf.bits, elf.machine)
            })?;
        }
        Ok(())
    }
    fn external(&mut self, member: &ExternalMember, control: &mut dyn RunControl) -> Result<()> {
        if let Capture::Unavailable { diagnostic } = &member.capture {
            self.diagnostic(diagnostic, control)?;
        }
        Ok(())
    }
}
impl ElfSink for Human<'_> {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, symbol: &SymbolRecord, control: &mut dyn RunControl) -> Result<()> {
        output(self.file, control, |writer| {
            writeln!(
                writer,
                "    symbol {}:{} {:?} bind={} type={} section={}",
                symbol.id.table_section,
                symbol.id.index,
                symbol.name.as_deref().map(String::from_utf8_lossy),
                symbol.binding,
                symbol.symbol_type,
                symbol.raw_section
            )
        })
    }
    fn diagnostic(&mut self, record: &Diagnostic, control: &mut dyn RunControl) -> Result<()> {
        output(self.file, control, |writer| {
            writeln!(
                writer,
                "  {:?}: {}: {}",
                record.code, record.context, record.message
            )
        })
    }
}
struct Doctor<'a> {
    file: &'a mut dyn Write,
    format: super::Format,
    errors: u64,
    unfinished: u64,
    switched: bool,
    started: bool,
}
impl Doctor<'_> {
    fn begin(&mut self, control: &mut dyn RunControl) -> Result<()> {
        if !self.started {
            if matches!(self.format, super::Format::Json) {
                raw(self.file, b"{\"schema\":2,\"errors\":[", control)?;
            }
            self.started = true;
        }
        Ok(())
    }
    fn switch(&mut self, control: &mut dyn RunControl) -> Result<()> {
        if !self.switched {
            raw(self.file, b"],\"unfinished_runs\":[", control)?;
            self.switched = true;
        }
        Ok(())
    }
}
impl app::DoctorSink for Doctor<'_> {
    fn error(&mut self, error: &Error, control: &mut dyn RunControl) -> Result<()> {
        self.begin(control)?;
        match self.format {
            super::Format::Json => {
                if self.errors != 0 {
                    raw(self.file, b",", control)?;
                }
                json(self.file, error, control)?;
            }
            super::Format::Human => output(self.file, control, |writer| {
                writeln!(writer, "{:?}: {}", error.code, error.message)
            })?,
        }
        self.errors += 1;
        Ok(())
    }
    fn unfinished(&mut self, id: &RunId, control: &mut dyn RunControl) -> Result<()> {
        self.begin(control)?;
        match self.format {
            super::Format::Json => {
                self.switch(control)?;
                if self.unfinished != 0 {
                    raw(self.file, b",", control)?;
                }
                json(self.file, id, control)?;
            }
            super::Format::Human => output(self.file, control, |writer| {
                writeln!(
                    writer,
                    "Unfinished run {id}; use recover after its owner exits"
                )
            })?,
        }
        self.unfinished += 1;
        Ok(())
    }
}

impl app::QuerySink for Human<'_> {
    fn summary(&mut self, summary: &app::QuerySummary, control: &mut dyn RunControl) -> Result<()> {
        let app::QuerySummary::Inventory {
            revision_id,
            complete,
        } = summary
        else {
            return Err(Error::new(
                ErrorCode::WorkerProtocol,
                "unexpected query summary",
            ));
        };
        output(self.file, control, |writer| {
            writeln!(
                writer,
                "Revision {} ({})",
                revision_id,
                if *complete {
                    "complete inventory"
                } else {
                    "incomplete inventory; inspect diagnostics"
                }
            )
        })
    }
}
impl app::DoctorSink for Human<'_> {
    fn error(&mut self, _: &Error, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::WorkerProtocol,
            "unexpected doctor record",
        ))
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::WorkerProtocol,
            "unexpected doctor record",
        ))
    }
}
impl ElfSink for Doctor<'_> {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::WorkerProtocol,
            "unexpected inventory record",
        ))
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::WorkerProtocol,
            "unexpected inventory record",
        ))
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::WorkerProtocol,
            "unexpected inventory record",
        ))
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::WorkerProtocol,
            "unexpected inventory record",
        ))
    }
}
impl InventorySink for Doctor<'_> {}
impl app::QuerySink for Doctor<'_> {
    fn summary(&mut self, summary: &app::QuerySummary, control: &mut dyn RunControl) -> Result<()> {
        let app::QuerySummary::Doctor {
            project,
            storage_schema,
            checked_revisions,
            checked_images,
            checked_analyses,
            checked_publications,
            ..
        } = summary
        else {
            return Err(Error::new(
                ErrorCode::WorkerProtocol,
                "unexpected query summary",
            ));
        };
        self.begin(control)?;
        if matches!(self.format, super::Format::Json) {
            self.switch(control)?;
            raw(self.file, b"],\"project\":", control)?;
            json(self.file, project, control)?;
            raw(self.file, b",\"storage_schema\":", control)?;
            json(self.file, storage_schema, control)?;
            raw(self.file, b",\"checked_revisions\":", control)?;
            json(self.file, checked_revisions, control)?;
            raw(self.file, b",\"checked_images\":", control)?;
            json(self.file, checked_images, control)?;
            raw(self.file, b",\"checked_analyses\":", control)?;
            json(self.file, checked_analyses, control)?;
            raw(self.file, b",\"checked_publications\":", control)?;
            json(self.file, checked_publications, control)?;
            raw(self.file, b",\"assessment\":", control)?;
            json(self.file, &summary.assessment(), control)?;
            raw(self.file, b"}\n", control)
        } else {
            output(self.file, control, |writer| {
                writeln!(
                    writer,
                    "Project {}; storage schema {}; verified revisions: {}; verified images: {}; verified analyses: {}; verified publications: {}",
                    project,
                    storage_schema,
                    checked_revisions,
                    checked_images,
                    checked_analyses,
                    checked_publications
                )
            })
        }
    }
}

/// Frontend-only framing; callbacks remain borrowed and bounded.
struct Records<'a> {
    file: &'a mut dyn Write,
    format: super::Format,
    count: u64,
    started: bool,
}
impl Records<'_> {
    fn begin(&mut self, c: &mut dyn RunControl) -> Result<()> {
        if !self.started {
            if matches!(self.format, super::Format::Json) {
                raw(self.file, b"{\"schema\":2,\"records\":[", c)?;
            }
            self.started = true;
        }
        Ok(())
    }
    fn record(
        &mut self,
        kind: &str,
        value: &impl serde::Serialize,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.begin(c)?;
        if matches!(self.format, super::Format::Json) {
            if self.count != 0 {
                raw(self.file, b",", c)?;
            }
            #[derive(serde::Serialize)]
            struct Record<'a, T> {
                kind: &'a str,
                value: &'a T,
            }
            json(self.file, &Record { kind, value }, c)?;
        } else {
            raw(self.file, kind.as_bytes(), c)?;
            raw(self.file, b": ", c)?;
            json(self.file, value, c)?;
            raw(self.file, b"\n", c)?;
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "record count overflow"))?;
        Ok(())
    }
}
impl InventorySink for Records<'_> {
    fn revision(&mut self, r: &RevisionHeader, c: &mut dyn RunControl) -> Result<()> {
        self.record("revision", r, c)
    }
    fn input(&mut self, n: u64, r: &InputRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("input", &(n, r), c)
    }
    fn object(&mut self, r: &ObjectInventory, c: &mut dyn RunControl) -> Result<()> {
        self.record("object", r, c)
    }
    fn external(&mut self, r: &ExternalMember, c: &mut dyn RunControl) -> Result<()> {
        self.record("external", r, c)
    }
}
impl ElfSink for Records<'_> {
    fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("section", r, c)
    }
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("symbol", r, c)
    }
    fn relocation(&mut self, r: &RelocationRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("relocation", r, c)
    }
    fn diagnostic(&mut self, r: &Diagnostic, c: &mut dyn RunControl) -> Result<()> {
        self.record("diagnostic", r, c)
    }
}
impl app::DoctorSink for Records<'_> {
    fn error(&mut self, r: &Error, c: &mut dyn RunControl) -> Result<()> {
        self.record("error", r, c)
    }
    fn unfinished(&mut self, r: &RunId, c: &mut dyn RunControl) -> Result<()> {
        self.record("unfinished", r, c)
    }
}
impl app::QuerySink for Records<'_> {
    fn trace(&mut self, r: &TraceRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("trace", r, c)
    }
    fn semantic_ir(&mut self, r: &SemanticIrRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("semantic-ir", r, c)
    }
    fn register(&mut self, r: &RegisterRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("register", r, c)
    }
    fn event_route(&mut self, r: &EventRouteRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("event-route", r, c)
    }
    fn memory_slice(&mut self, r: &MemorySliceRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("memory-slice", r, c)
    }
    fn flow(&mut self, r: &FlowRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("flow", r, c)
    }
    fn navigation(&mut self, r: &NavigationRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("navigation", r, c)
    }
    fn interface(&mut self, r: &InterfaceObservation, c: &mut dyn RunControl) -> Result<()> {
        self.record("interface", r, c)
    }
    fn coverage(&mut self, r: &ExtentCoverageRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("coverage", r, c)
    }
    fn data(&mut self, r: &DataRecord, c: &mut dyn RunControl) -> Result<()> {
        if matches!(self.format, super::Format::Human)
            && let DataRecord::Analysis {
                analysis,
                ordinal,
                record,
                ranges,
            } = r
        {
            return output(self.file, c, |w| {
                writeln!(
                    w,
                    "analysis={analysis} record={ordinal} data-ranges={ranges:?}"
                )?;
                super::function_display::record(w, record)
            });
        }
        self.record("data", r, c)
    }
    fn legacy(&mut self, r: &LegacyRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("legacy-record", r, c)
    }
    fn knowledge_entry(&mut self, r: &KnowledgeEntry, c: &mut dyn RunControl) -> Result<()> {
        self.record("assertion", r, c)
    }
    fn knowledge_event(&mut self, r: &KnowledgeEvent, c: &mut dyn RunControl) -> Result<()> {
        self.record("review-event", r, c)
    }
    fn investigation_entry(&mut self, r: &PlanEntry, c: &mut dyn RunControl) -> Result<()> {
        self.record("plan-entry", r, c)
    }
    fn investigation_member(
        &mut self,
        r: &InvestigationMember,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if matches!(self.format, super::Format::Human)
            && let PlanEntry::Function {
                name,
                declared_extent,
                address_space,
                ..
            } = &r.entry
        {
            return output(self.file, c, |w| {
                write!(
                    w,
                    "{:?}:{:08x} +{} {}",
                    address_space,
                    declared_extent.start,
                    declared_extent.length,
                    name.as_ref()
                        .map_or_else(|| "<unnamed>".into(), |n| String::from_utf8_lossy(n))
                )?;
                match &r.outcome {
                    InvestigationOutcome::Analyzed { analysis, complete } => writeln!(
                        w,
                        "  {}  analysis={analysis}",
                        if *complete { "complete" } else { "partial" }
                    ),
                    InvestigationOutcome::Blocked { error } => {
                        writeln!(w, "  blocked: {}", error.message)
                    }
                    InvestigationOutcome::Recorded => writeln!(w),
                }
            });
        }
        self.record("member", r, c)
    }
    fn publication(&mut self, r: &PublicationId, c: &mut dyn RunControl) -> Result<()> {
        self.record("publication", r, c)
    }
    fn finding(&mut self, r: &InvestigationFinding, c: &mut dyn RunControl) -> Result<()> {
        if matches!(self.format, super::Format::Human) {
            return output(self.file, c, |w| {
                writeln!(w, "Analysis {} ({:?})", r.analysis, r.request.source)?;
                super::function_display::record(w, &r.record)
            });
        }
        self.record("finding", r, c)
    }
    fn analysis(&mut self, id: &FunctionAnalysisId, c: &mut dyn RunControl) -> Result<()> {
        self.record("analysis", id, c)
    }
    fn execution_evidence(
        &mut self,
        record: &ExecutionEvidence,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.record("execution", record, c)
    }
    fn function_record(&mut self, record: &FunctionRecord, c: &mut dyn RunControl) -> Result<()> {
        if matches!(self.format, super::Format::Human) {
            output(self.file, c, |w| super::function_display::record(w, record))
        } else {
            self.record("function", record, c)
        }
    }
    fn image(&mut self, id: &PreparedImageId, c: &mut dyn RunControl) -> Result<()> {
        self.record("image", id, c)
    }
    fn link_observation(
        &mut self,
        r: &LinkObservationRecord,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.record("link-observation", r, c)
    }
    fn image_mapping(&mut self, mapping: &ImageMapping, c: &mut dyn RunControl) -> Result<()> {
        self.record("mapping", mapping, c)
    }
    fn target_audit(&mut self, r: &TargetAuditRecord, c: &mut dyn RunControl) -> Result<()> {
        self.record("target-audit", r, c)
    }
    fn candidate(&mut self, r: &SelectionCandidate, c: &mut dyn RunControl) -> Result<()> {
        self.record("candidate", r, c)
    }
    fn summary(&mut self, r: &app::QuerySummary, c: &mut dyn RunControl) -> Result<()> {
        if matches!(self.format, super::Format::Human) {
            match r {
                app::QuerySummary::InvestigationStatus { status } => {
                    return output(self.file, c, |w| {
                        writeln!(
                            w,
                            "Revision: {}",
                            status.revision.as_ref().map_or("none", RevisionId::as_str)
                        )?;
                        writeln!(
                            w,
                            "Publication: {} ({})",
                            status
                                .publication
                                .as_ref()
                                .map_or("none", PublicationId::as_str),
                            if status.publication.is_none() {
                                "not analyzed"
                            } else if status.current {
                                "current"
                            } else {
                                "stale"
                            }
                        )?;
                        writeln!(
                            w,
                            "Knowledge: {}",
                            status
                                .knowledge
                                .as_ref()
                                .map_or("none", KnowledgeRevisionId::as_str)
                        )?;
                        if let Some(coverage) = status.coverage {
                            write_coverage(w, coverage)?;
                        }
                        Ok(())
                    });
                }
                app::QuerySummary::Publication { id, manifest } => {
                    return output(self.file, c, |w| {
                        writeln!(w, "Publication: {id}")?;
                        write_coverage(w, manifest.coverage)
                    });
                }
                app::QuerySummary::InvestigationPlan { plan } => {
                    return output(self.file, c, |w| {
                        writeln!(
                            w,
                            "Plan {}: {} functions, {} entries",
                            plan.id, plan.recipe.functions, plan.recipe.entry_count
                        )
                    });
                }
                app::QuerySummary::Publications { count } => {
                    return output(self.file, c, |w| writeln!(w, "Publications: {count}"));
                }
                _ => (),
            }
        }
        self.begin(c)?;
        if matches!(self.format, super::Format::Human)
            && let app::QuerySummary::Analysis { manifest, .. } = r
        {
            output(self.file, c, |w| match manifest.semantics {
                None => writeln!(w, "Values and memory effects: unavailable"),
                Some(s) => writeln!(
                    w,
                    "Semantics: {}; known values {}/{}, known addresses {}/{}, gaps {}",
                    if s.complete { "complete" } else { "partial" },
                    s.known_values,
                    s.values,
                    s.known_addresses,
                    s.accesses,
                    s.gaps
                ),
            })?;
        }
        if matches!(self.format, super::Format::Json) {
            raw(self.file, b"],\"summary\":", c)?;
            json(self.file, r, c)?;
            raw(self.file, b",\"assessment\":", c)?;
            json(self.file, &r.assessment(), c)?;
            raw(self.file, b"}\n", c)
        } else {
            self.record("summary", r, c)
        }
    }
}

fn write_coverage(w: &mut dyn Write, c: InvestigationCoverage) -> std::io::Result<()> {
    writeln!(
        w,
        "Coverage: {}; inputs {}, objects {}, functions {} (analyzed {}, complete {}, blocked {}), gaps {}",
        if c.complete() { "complete" } else { "partial" },
        c.inputs,
        c.objects,
        c.functions,
        c.analyzed,
        c.complete_functions,
        c.blocked,
        c.gaps
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Full;
    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::StorageFull.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn json_delivery_preserves_filesystem_exhaustion_through_serde() {
        let error = json(&mut Full, &"x".repeat(2 * WORK_BLOCK), &mut || Ok(())).unwrap_err();
        assert_eq!(error.code, ErrorCode::DiskFull);
    }
}
