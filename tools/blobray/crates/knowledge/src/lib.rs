//! Pure semantic admission. The application supplies verified evidence and
//! streams accepted assertions; this crate cannot read, publish or run analysis.
use blobray_domain::*;
mod interfaces;
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
pub fn validate_proposal(p: &KnowledgeProposal) -> Result<()> {
    if p.evidence.is_empty() {
        return Err(invalid("a proposal requires retained supporting evidence"));
    }
    if p.occurrence
        .symbol
        .as_ref()
        .is_some_and(|s| s.object != p.occurrence.object)
    {
        return Err(invalid("symbol and object occurrence differ"));
    }
    match &p.claim {
        KnowledgeClaim::Interface { contract } => interfaces::validate(p, contract)?,
        KnowledgeClaim::IntegerTable {
            selector,
            layout,
            purpose,
            applicability,
        } => {
            if !matches!(selector, DataSelector::Section { offset, length, .. } if *length > 0 && offset.checked_add(*length).is_some())
                || layout.byte_length().is_none()
                || purpose.trim().is_empty()
                || applicability.trim().is_empty()
            {
                return Err(invalid(
                    "table requires valid integer layout, purpose and applicability",
                ));
            }
        }
        KnowledgeClaim::PointerTable {
            selector,
            layout,
            purpose,
            applicability,
        } => {
            if !matches!(selector, DataSelector::Section { offset, length, .. } if *length > 0 && offset.checked_add(*length).is_some())
                || layout.byte_length().is_none()
                || purpose.trim().is_empty()
                || applicability.trim().is_empty()
            {
                return Err(invalid(
                    "pointer table requires a valid captured range, layout, purpose and applicability",
                ));
            }
        }
        KnowledgeClaim::Constant {
            analysis,
            record,
            purpose,
            applicability,
            ..
        } => {
            if purpose.trim().is_empty()
                || applicability.trim().is_empty()
                || !p.evidence.contains(&EvidenceRef::Analysis {
                    analysis: analysis.clone(),
                    record: Some(*record),
                })
            {
                return Err(invalid(
                    "constant requires exact analysis record, purpose and applicability",
                ));
            }
        }
        KnowledgeClaim::MmioRegion { region } => {
            if region.name.trim().is_empty()
                || region.name.len() > 256
                || region.name.chars().any(char::is_control)
                || region.range.length == 0
                || region.range.end().is_none()
            {
                return Err(invalid("invalid MMIO region"));
            }
        }

        KnowledgeClaim::MmioRegister { register } => {
            if register.name.trim().is_empty()
                || register.name.len() > 256
                || register.name.chars().any(char::is_control)
                || !matches!(register.width, 1 | 2 | 4)
                || !register.address.is_multiple_of(u32::from(register.width))
                || u64::from(register.address) + u64::from(register.width) > 1u64 << 32
                || register.fields.len() > 32
            {
                return Err(invalid("invalid MMIO register"));
            }
            let mut mask = 0u64;
            for (i, field) in register.fields.iter().enumerate() {
                if field.name.trim().is_empty()
                    || field.name.len() > 256
                    || field.name.chars().any(char::is_control)
                    || field.width == 0
                    || u16::from(field.lsb) + u16::from(field.width) > u16::from(register.width) * 8
                    || register.fields[..i].iter().any(|f| f.name == field.name)
                {
                    return Err(invalid("invalid MMIO field"));
                }
                let bits = ((1u64 << field.width) - 1) << field.lsb;
                if mask & bits != 0 {
                    return Err(invalid("overlapping MMIO fields"));
                }
                mask |= bits;
            }
        }

        KnowledgeClaim::Name { name }
            if name.trim().is_empty()
                || name.len() > 4096
                || name.chars().any(char::is_control) =>
        {
            return Err(invalid(
                "display name must be nonempty and at most 4096 bytes without controls",
            ));
        }
        KnowledgeClaim::Hypothesis { text } if text.trim().is_empty() => {
            return Err(invalid("hypothesis text is empty"));
        }
        KnowledgeClaim::ExecutableRange { section, extent }
            if p.occurrence.symbol.is_some()
                || *section == 0
                || extent.length == 0
                || !extent.start.is_multiple_of(2)
                || extent.start.checked_add(extent.length).is_none() =>
        {
            return Err(invalid(
                "executable boundary requires a symbol-independent occurrence and valid aligned section range",
            ));
        }
        KnowledgeClaim::FunctionExtent { extent }
            if p.occurrence.symbol.is_none()
                || extent.length == 0
                || extent.start.checked_add(extent.length).is_none() =>
        {
            return Err(invalid(
                "function boundary requires an exact symbol and nonempty valid extent",
            ));
        }
        _ => (),
    }
    for reference in &p.evidence {
        if let EvidenceRef::Source { range, .. } = reference
            && (range.length == 0 || range.start.checked_add(range.length).is_none())
        {
            return Err(invalid("source evidence range is empty or overflows"));
        }
    }
    Ok(())
}
pub fn validate_change(change: &KnowledgeChange) -> Result<()> {
    if change.actor.trim().is_empty() || change.reason.trim().is_empty() {
        return Err(invalid("review actor and reason are required"));
    }
    if let KnowledgeAction::Propose { proposal } = &change.action {
        validate_proposal(proposal)?;
    }
    if let KnowledgeAction::Review {
        decision: ReviewDecision::Reject,
        supersedes: Some(_),
        ..
    } = &change.action
    {
        return Err(invalid(
            "rejection cannot replace another accepted assertion",
        ));
    }
    Ok(())
}
/// Returns whether simultaneous acceptance would give incompatible interpretations.
/// Hypotheses may coexist and always retain their hypothesis evidence class.
pub fn conflicts(a: &KnowledgeProposal, b: &KnowledgeProposal) -> bool {
    if a.occurrence.revision != b.occurrence.revision {
        return false;
    }
    match (&a.claim, &b.claim) {
        (KnowledgeClaim::Interface { contract: x }, KnowledgeClaim::Interface { contract: y }) => {
            a.occurrence.source == b.occurrence.source
                && a.occurrence.object == b.occurrence.object
                && (a.subject == b.subject || interfaces::overlap(x, y))
                && x != y
        }
        (
            KnowledgeClaim::IntegerTable { selector: x, .. }
            | KnowledgeClaim::PointerTable { selector: x, .. },
            KnowledgeClaim::IntegerTable { selector: y, .. }
            | KnowledgeClaim::PointerTable { selector: y, .. },
        ) => {
            let overlapping = match (x, y) {
                (
                    DataSelector::Section {
                        section: a,
                        offset: x,
                        length: nx,
                    },
                    DataSelector::Section {
                        section: b,
                        offset: y,
                        length: ny,
                    },
                ) => a == b && *x < y.saturating_add(*ny) && *y < x.saturating_add(*nx),
                _ => false,
            };
            a.occurrence.source == b.occurrence.source
                && a.occurrence.object == b.occurrence.object
                && (overlapping || a.subject == b.subject)
                && a.claim != b.claim
        }
        (KnowledgeClaim::Constant { .. }, KnowledgeClaim::Constant { .. }) => {
            a.occurrence == b.occurrence && a.subject == b.subject && a.claim != b.claim
        }
        (KnowledgeClaim::MmioRegion { region: x }, KnowledgeClaim::MmioRegion { region: y }) => {
            a.occurrence.source == b.occurrence.source
                && a.occurrence.object == b.occurrence.object
                && x != y
                && (x.name == y.name
                    || (u64::from(x.range.start) < y.range.end().unwrap_or(0)
                        && u64::from(y.range.start) < x.range.end().unwrap_or(0)))
        }

        (
            KnowledgeClaim::MmioRegister { register: a_reg },
            KnowledgeClaim::MmioRegister { register: b_reg },
        ) => {
            a.occurrence.source == b.occurrence.source
                && a.occurrence.object == b.occurrence.object
                && ((u64::from(a_reg.address) < u64::from(b_reg.address) + u64::from(b_reg.width)
                    && u64::from(b_reg.address)
                        < u64::from(a_reg.address) + u64::from(a_reg.width))
                    || a_reg.name == b_reg.name)
                && a_reg != b_reg
        }

        (KnowledgeClaim::Binding, KnowledgeClaim::Binding) => {
            a.occurrence == b.occurrence && a.subject != b.subject
        }
        (KnowledgeClaim::Name { name: a_name }, KnowledgeClaim::Name { name: b_name }) => {
            a.subject == b.subject && a_name != b_name
        }
        (
            KnowledgeClaim::ExecutableRange {
                section: a_section,
                extent: a_extent,
            },
            KnowledgeClaim::ExecutableRange {
                section: b_section,
                extent: b_extent,
            },
        ) => {
            a.occurrence == b.occurrence
                && a_section == b_section
                && a_extent != b_extent
                && (a.subject == b.subject
                    || (a_extent.start < b_extent.start.saturating_add(b_extent.length)
                        && b_extent.start < a_extent.start.saturating_add(a_extent.length)))
        }
        (
            KnowledgeClaim::FunctionExtent { extent: a_extent },
            KnowledgeClaim::FunctionExtent { extent: b_extent },
        ) => a.occurrence == b.occurrence && a_extent != b_extent,
        _ => false,
    }
}
pub fn validate_review(
    target: &KnowledgeEntry,
    decision: ReviewDecision,
    replaced: Option<&KnowledgeEntry>,
) -> Result<()> {
    if target.state != AssertionState::Proposed {
        return Err(Error::new(
            ErrorCode::Conflict,
            "only a pending proposal can be reviewed",
        ));
    }
    if let Some(old) = replaced
        && (decision != ReviewDecision::Accept
            || old.state != AssertionState::Accepted
            || !conflicts(&target.proposal, &old.proposal))
    {
        return Err(invalid(
            "replacement must resolve a conflicting accepted assertion",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn proposal() -> KnowledgeProposal {
        let payload = ArtifactId::of_bytes(b"code");
        let object = ObjectId {
            artifact: payload.clone(),
            location: ObjectLocation::Standalone,
        };
        KnowledgeProposal {
            subject: "entry".to_owned().try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: payload.as_str().parse().unwrap(),
                source: FunctionSource::Input { input: 0 },
                object,
                symbol: None,
            },
            claim: KnowledgeClaim::Name { name: "one".into() },
            evidence: vec![EvidenceRef::Source {
                payload,
                range: CodeRange {
                    start: 0,
                    length: 4,
                },
            }],
            note: None,
        }
    }
    #[test]
    fn contradictory_names_conflict_but_hypotheses_do_not_become_facts() {
        let a = proposal();
        let mut b = a.clone();
        b.claim = KnowledgeClaim::Name { name: "two".into() };
        assert!(conflicts(&a, &b));
        b.claim = KnowledgeClaim::Hypothesis {
            text: "possible behavior".into(),
        };
        assert!(!conflicts(&a, &b));
        validate_proposal(&b).unwrap();
        b.evidence.clear();
        assert!(validate_proposal(&b).is_err());
    }
    #[test]
    fn occurrence_binding_is_revision_and_input_qualified() {
        let mut a = proposal();
        a.claim = KnowledgeClaim::Binding;
        let mut b = a.clone();
        b.subject = "another".to_owned().try_into().unwrap();
        assert!(conflicts(&a, &b));
        b.occurrence.source = FunctionSource::Input { input: 1 };
        assert!(!conflicts(&a, &b));
    }
    #[test]
    fn mmio_fields_ranges_and_applicability_are_validated() {
        let mut a = proposal();
        a.claim = KnowledgeClaim::MmioRegister {
            register: MmioRegister {
                name: "CONTROL".into(),
                address: 0x60000000,
                width: 4,
                fields: vec![MmioField {
                    name: "ENABLE".into(),
                    lsb: 16,
                    width: 1,
                }],
            },
        };
        validate_proposal(&a).unwrap();
        let mut bad = a.clone();
        if let KnowledgeClaim::MmioRegister { register } = &mut bad.claim {
            register.fields.push(MmioField {
                name: "OTHER".into(),
                lsb: 16,
                width: 2,
            });
        }
        assert!(validate_proposal(&bad).is_err());
        let mut b = a.clone();
        if let KnowledgeClaim::MmioRegister { register } = &mut b.claim {
            register.name = "OTHER".into();
        }
        assert!(conflicts(&a, &b));
        b.occurrence.source = FunctionSource::Image {
            image: ArtifactId::of_bytes(b"image").as_str().parse().unwrap(),
        };
        assert!(!conflicts(&a, &b));
        a.claim = KnowledgeClaim::MmioRegion {
            region: MmioRegion {
                name: "RADIO".into(),
                range: ImageRegion {
                    start: 0xfffffff0,
                    length: 32,
                },
            },
        };
        assert!(validate_proposal(&a).is_err());
    }
    #[test]
    fn executable_boundaries_conflict_only_with_differing_overlapping_section_claims() {
        let mut a = proposal();
        a.claim = KnowledgeClaim::ExecutableRange {
            section: 1,
            extent: CodeRange {
                start: 8,
                length: 8,
            },
        };
        validate_proposal(&a).unwrap();
        let mut b = a.clone();
        assert!(!conflicts(&a, &b));
        b.subject = "second".to_owned().try_into().unwrap();
        b.claim = KnowledgeClaim::ExecutableRange {
            section: 1,
            extent: CodeRange {
                start: 12,
                length: 8,
            },
        };
        assert!(conflicts(&a, &b));
        b.claim = KnowledgeClaim::ExecutableRange {
            section: 1,
            extent: CodeRange {
                start: 16,
                length: 8,
            },
        };
        assert!(!conflicts(&a, &b));
        b.claim = KnowledgeClaim::ExecutableRange {
            section: 2,
            extent: CodeRange {
                start: 8,
                length: 8,
            },
        };
        assert!(!conflicts(&a, &b));
        b.occurrence.symbol = Some(SymbolId {
            object: b.occurrence.object.clone(),
            table: SymbolTableKind::Static,
            table_section: 3,
            index: 1,
        });
        assert!(validate_proposal(&b).is_err());
    }
    #[test]
    fn differing_pointer_and_integer_layouts_conflict_on_the_same_captured_bytes() {
        let mut a = proposal();
        let selector = DataSelector::Section {
            section: 1,
            offset: 0,
            length: 8,
        };
        a.claim = KnowledgeClaim::PointerTable {
            selector: selector.clone(),
            layout: PointerTable {
                count: 2,
                stride: 4,
            },
            purpose: "pointers".into(),
            applicability: "fixture".into(),
        };
        validate_proposal(&a).unwrap();
        let mut b = a.clone();
        b.subject = "integer-view".to_owned().try_into().unwrap();
        b.claim = KnowledgeClaim::IntegerTable {
            selector,
            layout: IntegerTable {
                encoding: IntegerEncoding {
                    width: 4,
                    signed: false,
                    byte_order: DataByteOrder::Little,
                },
                count: 2,
                stride: 4,
            },
            purpose: "raw words".into(),
            applicability: "fixture".into(),
        };
        validate_proposal(&b).unwrap();
        assert!(conflicts(&a, &b));
        assert!(conflicts(&b, &a));
    }
}
