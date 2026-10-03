//! Every function captured libraries define, analyzed in process.
use crate::captured::{Member, visit_members};
use crate::in_process::Executable;
use crate::*;
use blobray_analysis::navigation::Facts;

/// One analyzed function with its records, borrowed for a visit.
pub struct AnalyzedFunction<'a> {
    pub function: &'a LibraryFunction,
    pub records: &'a [FunctionRecord],
    pub coverage: FunctionCoverage,
    pub semantics: SemanticSummary,
}
impl AnalyzedFunction<'_> {
    /// Coverage and value semantics are complete.
    pub fn complete(&self) -> bool {
        self.coverage.complete() && self.semantics.complete
    }
}

/// One outcome of a library analysis, in input, object and symbol order.
pub enum LibraryOutcome<'a> {
    Analyzed(AnalyzedFunction<'a>),
    /// A function that cannot be analyzed; its behavior is unknown.
    Blocked {
        function: &'a LibraryFunction,
        error: &'a Error,
    },
    /// Code no function is selected or analyzed from.
    Gap {
        input: u64,
        object: Option<&'a ObjectId>,
        reason: &'a str,
    },
}

type Visit<'v> = dyn FnMut(LibraryOutcome<'_>, &mut dyn RunControl) -> Result<()> + 'v;

/// Errors that block one function's analysis without failing the others.
fn blocks_function(error: &Error) -> bool {
    matches!(
        error.code,
        ErrorCode::NeedsExtent
            | ErrorCode::Incompatible
            | ErrorCode::InvalidRequest
            | ErrorCode::Unavailable
    )
}

/// Analyze every function symbol of an executable section of every object
/// `inputs` contain, presenting each outcome to `visit`.
pub fn analyze_library(
    inputs: &[Executable],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<()> {
    for (index, executable) in inputs.iter().enumerate() {
        let input = index as u64;
        let mut position = RunPosition {
            input: Some(input),
            ..RunPosition::default()
        };
        position.artifact(executable.id());
        control.set_position(position);
        let container = visit_members(executable, memory, control, &mut |member, c| {
            analyze_object(input, member, decoder, memory, c, visit)
        })?;
        let gap = |reason: &str, visit: &mut Visit<'_>, c: &mut dyn RunControl| {
            visit(
                LibraryOutcome::Gap {
                    input,
                    object: None,
                    reason,
                },
                c,
            )
        };
        if container.framing.is_some() {
            gap("archive member enumeration incomplete", visit, control)?;
        }
        if container.kind == ContainerKind::Unsupported {
            gap("unsupported input format", visit, control)?;
        }
    }
    Ok(())
}

/// The sections and function symbols of one object.
#[derive(Default)]
struct Selection {
    /// Indices of nonempty executable sections, sorted.
    executable: Vec<u32>,
    functions: Vec<SymbolRecord>,
    diagnostics: Vec<Diagnostic>,
}
impl ElfSink for Selection {
    fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        if r.flags & 4 != 0 && r.size != 0 {
            match self.executable.binary_search(&r.index) {
                Ok(_) => return Err(Error::new(ErrorCode::Integrity, "duplicate section index")),
                Err(i) => self.executable.insert(i, r.index),
            }
        }
        c.checkpoint(1)
    }
    fn symbol(&mut self, r: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        // STT_FUNC in a section already known to be executable.
        let section = if r.raw_section == 0xffff {
            r.extended_section
        } else if r.raw_section > 0 && r.raw_section < 0xff00 {
            Some(u32::from(r.raw_section))
        } else {
            None
        };
        if r.symbol_type == 2 && section.is_some_and(|s| self.executable.binary_search(&s).is_ok())
        {
            self.functions.push(r.clone());
        }
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, r: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        self.diagnostics.push(r.clone());
        Ok(())
    }
}

/// Research the function `symbol` defines in the prepared `object` into
/// `sink`. `references` keeps each section's prepared relocations for the
/// object's other functions.
#[allow(clippy::too_many_arguments)]
fn research_function<'m>(
    object: &mut blobray_artifacts::PreparedObject<'_, '_>,
    references: &mut AdmittedVec<'m, (u32, oer_riscv_analysis::PreparedReferences<'m>)>,
    symbol: &SymbolId,
    input: u64,
    payload: &ArtifactId,
    decoder: &dyn FunctionSemantics,
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
) -> Result<oer_riscv_analysis::AnalysisSummary> {
    let mut position = RunPosition {
        phase: RunPhase::AnalyzeFunction,
        input: Some(input),
        member: match symbol.object.location {
            ObjectLocation::Standalone => None,
            ObjectLocation::ArchiveMember { ordinal } => Some(ordinal),
        },
        ..Default::default()
    };
    position.artifact(payload);
    control.set_position(position);
    control.checkpoint(0)?;
    object.with_function(symbol, control, |view, control| {
        let index = if let Some(index) = references
            .iter()
            .position(|(section, _)| *section == view.section)
        {
            index
        } else {
            let prepared = oer_riscv_analysis::PreparedReferences::new(
                view.relocations,
                view.section,
                decoder,
                memory,
                control,
            )?;
            references.push((view.section, prepared), control.position())?;
            references.len() - 1
        };
        oer_riscv_analysis::research(
            oer_riscv_analysis::FunctionInput {
                image: view.image,
                section: view.section,
                extent: view.extent,
                bytes: view.code,
                relocations: &references[index].1,
                data_ranges: view.data_ranges,
                jumps: &[],
            },
            decoder,
            memory,
            control,
            sink,
            None,
        )
    })
}

/// Collects one function's records into admitted memory.
struct Records<'a, 'm>(&'a mut RecordBuffer<'m>);
impl FunctionSink for Records<'_, '_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        control.checkpoint(1)?;
        self.0.push(record.clone(), control.position())
    }
}

fn analyze_object(
    input: u64,
    member: Member<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<()> {
    let object = member.id;
    let mut selection = Selection::default();
    let (bytes, payload, elf) = match member.bytes {
        Ok(bytes) => {
            let (payload, elf) = inspect_source(bytes, &object, memory, control, &mut selection)?;
            (Some(bytes), Some(payload), elf)
        }
        Err(diagnostic) => {
            selection.diagnostics.push(diagnostic);
            (None, None, None)
        }
    };
    let mut gap = |reason: &str, c: &mut dyn RunControl| {
        visit(
            LibraryOutcome::Gap {
                input,
                object: Some(&object),
                reason,
            },
            c,
        )
    };
    if payload.is_none() {
        gap("object payload unavailable", control)?;
    }
    if !elf.as_ref().is_some_and(|e| {
        e.bits == 32 && e.little_endian && e.machine == 243 && matches!(e.object_type, 1 | 2)
    }) {
        gap(
            "object is not a supported ELF32 little-endian RISC-V relocatable or executable",
            control,
        )?;
    }
    for d in &selection.diagnostics {
        gap(
            &format!("{:?}: {}: {}", d.code, d.context, d.message),
            control,
        )?;
    }
    if selection.functions.is_empty()
        && (!selection.executable.is_empty() || elf.as_ref().is_some_and(|e| e.object_type == 2))
    {
        gap(
            "executable sections have no selected function symbols",
            control,
        )?;
    }
    let (Some(bytes), Some(payload)) = (bytes, payload) else {
        return Ok(());
    };
    let functions: Vec<LibraryFunction> = selection
        .functions
        .into_iter()
        .map(|r| LibraryFunction {
            input,
            symbol: r.id,
            name: r.name,
        })
        .collect();
    if functions.is_empty() {
        return Ok(());
    }
    let mut entered = false;
    let result =
        blobray_artifacts::with_prepared_object(bytes, &payload, memory, control, |prepared, c| {
            entered = true;
            let mut references = AdmittedVec::new(memory);
            for function in &functions {
                let mut records = RecordBuffer::new(memory);
                let researched = research_function(
                    prepared,
                    &mut references,
                    &function.symbol,
                    input,
                    &payload,
                    decoder,
                    memory,
                    c,
                    &mut Records(&mut records),
                );
                match researched {
                    Ok(summary) => visit(
                        LibraryOutcome::Analyzed(AnalyzedFunction {
                            function,
                            records: &records,
                            coverage: summary.coverage,
                            semantics: summary.semantics,
                        }),
                        c,
                    )?,
                    Err(error) if blocks_function(&error) => visit(
                        LibraryOutcome::Blocked {
                            function,
                            error: &error,
                        },
                        c,
                    )?,
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        });
    match result {
        Err(error) if !entered && blocks_function(&error) => {
            for function in &functions {
                visit(
                    LibraryOutcome::Blocked {
                        function,
                        error: &error,
                    },
                    control,
                )?;
            }
            Ok(())
        }
        other => other,
    }
}

/// Every candidate memory address the functions `inputs` define access,
/// inside `ranges` when it is nonempty, with blocked functions and gaps.
pub fn register_accesses(
    inputs: &[Executable],
    ranges: &[ImageRegion],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    emit: &mut dyn FnMut(&RegisterAccess, &mut dyn RunControl) -> Result<()>,
) -> Result<RegisterAccessSummary> {
    crate::registers::validate_ranges(ranges)?;
    let mut summary = RegisterAccessSummary {
        schema: 1,
        ranges: ranges.to_vec(),
        functions: 0,
        partial_functions: 0,
        blocked_functions: 0,
        gaps: 0,
        observations: 0,
        unresolved_addresses: 0,
    };
    analyze_library(
        inputs,
        decoder,
        memory,
        control,
        &mut |outcome, c| match outcome {
            LibraryOutcome::Analyzed(analyzed) => {
                summary.functions += 1;
                summary.partial_functions += u64::from(!analyzed.complete());
                c.phase(RunPhase::AnalyzeValues)?;
                let facts = Facts::new(analyzed.records, memory, c)?;
                crate::registers::observe(analyzed.records, &facts, ranges, c, &mut |o, c| {
                    summary.observations += 1;
                    summary.unresolved_addresses += u64::from(o.address.is_none());
                    emit(
                        &RegisterAccess::Observation {
                            function: analyzed.function.clone(),
                            record: o.record,
                            fact: Box::new(o.fact.clone()),
                            address: o.address,
                            alternative: o.alternative,
                            mask: o.mask,
                        },
                        c,
                    )
                })
            }
            LibraryOutcome::Blocked { function, error } => {
                summary.blocked_functions += 1;
                emit(
                    &RegisterAccess::Blocked {
                        function: function.clone(),
                        error: error.clone(),
                    },
                    c,
                )
            }
            LibraryOutcome::Gap {
                input,
                object,
                reason,
            } => {
                summary.gaps += 1;
                emit(
                    &RegisterAccess::Gap {
                        input,
                        object: object.cloned(),
                        reason: reason.to_owned(),
                    },
                    c,
                )
            }
        },
    )?;
    Ok(summary)
}
