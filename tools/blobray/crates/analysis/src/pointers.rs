//! Streaming interpretation of explicitly selected pointer slots. No resolution
//! by name, project reads, relocation application or retained per-table allocation.
use blobray_domain::*;
pub struct PointerInput<'a> {
    pub bytes: &'a [u8],
    /// Coordinate used by physical relocation records (VMA for ET_EXEC).
    pub start: u64,
    pub image: bool,
    pub unknown_write_extents: bool,
    pub max_write_bytes: u8,
    /// Sorted physical section relocations, borrowed from the prepared owner.
    pub relocations: &'a [FunctionRelocation],
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn issue(issue: PointerIssue) -> PointerValue {
    PointerValue::Unresolved { issue }
}
fn address(value: u32, image: bool) -> PointerValue {
    if value == 0 {
        PointerValue::Null
    } else {
        PointerValue::Address {
            value,
            image_address: image,
        }
    }
}
fn interpret(r: &FunctionRelocation, image: bool, bits: u32) -> PointerValue {
    let Some(addend) = r.addend else {
        return issue(PointerIssue::ImplicitAddend);
    };
    match r.target.definition {
        SymbolDefinition::Undefined => PointerValue::ExternalSymbol {
            symbol: r.target.symbol.clone(),
            addend,
        },
        SymbolDefinition::Section => {
            if image
                && u32::try_from(r.target.offset)
                    .ok()
                    .map(|base| base.wrapping_add(addend as u32))
                    != Some(bits)
            {
                return issue(PointerIssue::LinkedValueMismatch);
            }
            PointerValue::DefinedSymbol {
                symbol: r.target.symbol.clone(),
                addend,
            }
        }
        SymbolDefinition::Null | SymbolDefinition::Absolute => {
            let Some(value) = u32::try_from(r.target.offset)
                .ok()
                .map(|base| base.wrapping_add(addend as u32))
            else {
                return issue(PointerIssue::ArithmeticOverflow);
            };
            if image && value != bits {
                return issue(PointerIssue::LinkedValueMismatch);
            }
            address(value, image)
        }
        _ => issue(PointerIssue::UnsupportedSymbol),
    }
}
pub fn analyze(
    input: PointerInput<'_>,
    layout: &PointerTable,
    decoder: &dyn PointerDecoder,
    control: &mut dyn RunControl,
    emit: &mut dyn FnMut(&DataRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<PointerSummary> {
    if decoder.pointer_identity().is_none() {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "pointer relocation profile unavailable",
        ));
    }
    if layout.byte_length() != Some(input.bytes.len() as u64) {
        return Err(invalid("pointer layout differs from captured range"));
    }
    control.phase(RunPhase::AnalyzeValues)?;
    let mut summary = PointerSummary::default();
    for index in 0..layout.count {
        control.checkpoint(1)?;
        let offset = index
            .checked_mul(layout.stride)
            .ok_or_else(|| invalid("pointer offset overflow"))?;
        let pos = usize::try_from(offset)
            .map_err(|_| invalid("pointer offset exceeds host address space"))?;
        let bits = u32::from_le_bytes(
            input
                .bytes
                .get(pos..pos + 4)
                .ok_or_else(|| invalid("pointer slot outside range"))?
                .try_into()
                .unwrap(),
        );
        let at = input
            .start
            .checked_add(offset)
            .ok_or_else(|| invalid("pointer address overflow"))?;
        let end = at
            .checked_add(4)
            .ok_or_else(|| invalid("pointer slot overflow"))?;
        let value = if input.unknown_write_extents {
            issue(PointerIssue::UnknownWriteExtent)
        } else {
            control.checkpoint(input.relocations.len().max(1).ilog2() as u64 + 1)?;
            control.measure(WorkMetric::RelocationLookups, 1);
            let first = input.relocations.partition_point(|r| {
                r.offset.saturating_add(u64::from(input.max_write_bytes)) <= at
            });
            let mut found = None;
            let mut overlaps = 0;
            for r in input.relocations[first..]
                .iter()
                .take_while(|r| r.offset < end)
            {
                control.checkpoint(1)?;
                let (width, supported) = match decoder.pointer_relocation(r) {
                    PointerRelocation::None => continue,
                    PointerRelocation::Absolute32 => (4, true),
                    PointerRelocation::Unsupported { width } => {
                        (width.unwrap_or(input.max_write_bytes), false)
                    }
                };
                if r.offset.saturating_add(u64::from(width)) <= at {
                    continue;
                }
                overlaps += 1;
                found = Some(if !supported {
                    issue(PointerIssue::UnsupportedRelocation)
                } else if r.offset != at {
                    issue(PointerIssue::PartialSlotWrite)
                } else {
                    interpret(r, input.image, bits)
                });
            }
            if overlaps > 1 {
                issue(PointerIssue::OverlappingRelocations)
            } else {
                found.unwrap_or_else(|| address(bits, input.image))
            }
        };
        summary.entries += 1;
        match &value {
            PointerValue::Null => summary.nulls += 1,
            PointerValue::Address { .. } => summary.addresses += 1,
            PointerValue::DefinedSymbol { .. } => summary.defined_symbols += 1,
            PointerValue::ExternalSymbol { .. } => summary.external_symbols += 1,
            PointerValue::Unresolved { .. } => summary.unresolved += 1,
        }
        emit(
            &DataRecord::Pointer {
                index,
                offset,
                bits,
                value,
            },
            control,
        )?;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    struct Absolute32;
    impl PointerDecoder for Absolute32 {
        fn pointer_identity(&self) -> Option<&'static str> {
            Some("fixture-absolute32/1")
        }
        fn pointer_relocation(&self, _: &FunctionRelocation) -> PointerRelocation {
            PointerRelocation::Absolute32
        }
    }
    fn pointers(records: &[DataRecord]) -> Vec<&PointerValue> {
        records
            .iter()
            .filter_map(|r| match r {
                DataRecord::Pointer { value, .. } => Some(value),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn pointer_profile_uses_modulo_32_arithmetic_and_rejects_implicit_or_stale_linked_words() {
        let object = ObjectId {
            artifact: ArtifactId::of_bytes(b"pointer-fixture"),
            location: ObjectLocation::Standalone,
        };
        for (base, addend, image, bits, expected) in [
            (
                0,
                Some(-1),
                false,
                0,
                PointerValue::Address {
                    value: u32::MAX,
                    image_address: false,
                },
            ),
            (
                u64::from(u32::MAX),
                Some(2),
                false,
                0,
                PointerValue::Address {
                    value: 1,
                    image_address: false,
                },
            ),
            (
                16,
                Some(4),
                true,
                20,
                PointerValue::Address {
                    value: 20,
                    image_address: true,
                },
            ),
            (
                16,
                Some(4),
                true,
                16,
                PointerValue::Unresolved {
                    issue: PointerIssue::LinkedValueMismatch,
                },
            ),
            (
                16,
                None,
                false,
                4,
                PointerValue::Unresolved {
                    issue: PointerIssue::ImplicitAddend,
                },
            ),
        ] {
            let raw = FunctionRelocation {
                section: 3,
                index: 0,
                offset: 0,
                relocation_type: 1234,
                addend,
                target: Arc::new(ReferenceTarget {
                    symbol: SymbolId {
                        object: object.clone(),
                        table: SymbolTableKind::Static,
                        table_section: 2,
                        index: 1,
                    },
                    name: b"absolute".to_vec(),
                    section: None,
                    offset: base,
                    binding: 1,
                    definition: SymbolDefinition::Absolute,
                    symbol_type: 0,
                }),
            };
            let mut records = Vec::new();
            let bits: u32 = bits;
            analyze(
                PointerInput {
                    bytes: &bits.to_le_bytes(),
                    start: 0,
                    image,
                    unknown_write_extents: false,
                    max_write_bytes: 4,
                    relocations: &[raw],
                },
                &PointerTable {
                    count: 1,
                    stride: 4,
                },
                &Absolute32,
                &mut || Ok(()),
                &mut |r, _| {
                    records.push(r.clone());
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(pointers(&records), [&expected]);
        }
    }

    #[test]
    fn pointer_lookup_work_grows_with_slots_without_rescanning_all_relocations() {
        struct Counter(u64);
        impl RunControl for Counter {
            fn checkpoint(&mut self, units: u64) -> Result<()> {
                self.0 += units;
                Ok(())
            }
        }
        let target = Arc::new(ReferenceTarget {
            symbol: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"code"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 2,
                index: 1,
            },
            name: vec![],
            section: Some(1),
            offset: 0,
            binding: 1,
            definition: SymbolDefinition::Section,
            symbol_type: 2,
        });
        let mut charges = Vec::new();
        for count in [128, 1024] {
            let relocations: Vec<_> = (0..count)
                .map(|i| FunctionRelocation {
                    section: 3,
                    index: i,
                    offset: i * 4,
                    relocation_type: 1234,
                    addend: Some(0),
                    target: target.clone(),
                })
                .collect();
            let bytes = vec![0; count as usize * 4];
            let mut control = Counter(0);
            let mut emitted = 0;
            let summary = analyze(
                PointerInput {
                    bytes: &bytes,
                    start: 0,
                    image: false,
                    unknown_write_extents: false,
                    max_write_bytes: 4,
                    relocations: &relocations,
                },
                &PointerTable { count, stride: 4 },
                &Absolute32,
                &mut control,
                &mut |_, _| {
                    emitted += 1;
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(summary.defined_symbols, count);
            assert_eq!(emitted, count);
            charges.push(control.0);
        }
        assert!(charges[1] > charges[0] * 8);
        assert!(charges[1] < charges[0] * 12, "{charges:?}");
    }
}
