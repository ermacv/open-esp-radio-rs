//! Pure declaration admission. Runtime guards never become observed facts here.
use super::*;
fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}
fn value_type(value: &InterfaceValueType, result: bool) -> bool {
    match value {
        InterfaceValueType::Void => result,
        InterfaceValueType::Integer { bits, .. } => matches!(bits, 8 | 16 | 32 | 64),
        InterfaceValueType::Pointer { .. } => true,
    }
}
pub(super) fn validate(p: &KnowledgeProposal, d: &InterfaceContract) -> Result<()> {
    if !text(&d.layout_version)
        || !text(&d.purpose)
        || !text(&d.applicability)
        || d.layout_bytes < 4
        || d.slots.is_empty()
        || d.slots.len() > 64
        || d.path.len() > 16
        || d.guards.len() > 16
        || d.index_domains.len() > 8
    {
        return Err(invalid(
            "interface needs bounded layout, slots, path, purpose and applicability",
        ));
    }
    if d.pointer_bytes != 4 || d.abi != CallAbi::RiscvInteger {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "interface profile requires RV32 integer ABI and four-byte pointers",
        ));
    }
    match &d.root {
        InterfaceRoot::Section { section, offset }
            if *section == 0 || offset.checked_add(u64::from(d.layout_bytes)).is_none() =>
        {
            return Err(invalid("invalid section interface root"));
        }
        InterfaceRoot::Symbol { symbol, .. }
            if symbol.object != p.occurrence.object
                || p.occurrence.symbol.as_ref().is_some_and(|s| s != symbol) =>
        {
            return Err(invalid("interface symbol root differs from occurrence"));
        }
        InterfaceRoot::FunctionArgument { function, argument }
            if function.object() != &p.occurrence.object
                || *argument >= 32
                || p.occurrence
                    .symbol
                    .as_ref()
                    .is_some_and(|s| function.symbol() != Some(s)) =>
        {
            return Err(invalid(
                "interface argument root differs from captured function or exceeds profile",
            ));
        }
        _ => (),
    }
    for (i, domain) in d.index_domains.iter().enumerate() {
        if domain.argument >= 32 || domain.min > domain.max || !text(&domain.reason)
            || d.index_domains[..i].iter().any(|x|x.argument == domain.argument)
            || !d.path.iter().any(|s|matches!(s,InterfaceStep::Index { argument, .. } if *argument == domain.argument)) {
            return Err(invalid("interface index domains must be unique, bounded and used by the path"));
        }
    }
    for step in &d.path {
        match step {
            InterfaceStep::LoadPointer { offset } if *offset % 4 != 0 => {
                return Err(invalid("interface pointer load must be aligned"));
            }
            InterfaceStep::Index { argument, stride } => {
                let domain = d
                    .index_domains
                    .iter()
                    .find(|d| d.argument == *argument)
                    .ok_or_else(|| invalid("interface index has no declared domain"))?;
                if *stride == 0
                    || !stride.is_multiple_of(4)
                    || domain.max.checked_mul(*stride).is_none()
                {
                    return Err(invalid(
                        "interface indexed displacement is unaligned or overflows RV32",
                    ));
                }
            }
            _ => (),
        }
    }
    // Literal roots have provable address bounds; pointer loads end that proof.
    let mut bounds = match d.root {
        InterfaceRoot::Address { address } => Some((i64::from(address), i64::from(address))),
        _ => None,
    };
    for step in &d.path {
        if let Some((low, high)) = bounds {
            let (low, high, loaded) = match step {
                InterfaceStep::Offset { bytes } => {
                    (low + i64::from(*bytes), high + i64::from(*bytes), false)
                }
                InterfaceStep::Index { argument, stride } => {
                    let domain = d
                        .index_domains
                        .iter()
                        .find(|d| d.argument == *argument)
                        .expect("domain validated above");
                    (
                        low + i64::from(domain.min) * i64::from(*stride),
                        high + i64::from(domain.max) * i64::from(*stride),
                        false,
                    )
                }
                InterfaceStep::LoadPointer { offset } => {
                    (low + i64::from(*offset), high + i64::from(*offset), true)
                }
            };
            if low < 0 || high > i64::from(u32::MAX) || loaded && high + 4 > 1i64 << 32 {
                return Err(invalid("interface path crosses the RV32 address space"));
            }
            bounds = if loaded { None } else { Some((low, high)) };
        }
    }
    if bounds.is_some_and(|(_, high)| high + i64::from(d.layout_bytes) > 1i64 << 32) {
        return Err(invalid("interface layout crosses the RV32 address space"));
    }
    for (i, slot) in d.slots.iter().enumerate() {
        if !text(&slot.name)
            || !slot.offset.is_multiple_of(4)
            || slot
                .offset
                .checked_add(4)
                .is_none_or(|end| end > d.layout_bytes)
            || d.slots[..i]
                .iter()
                .any(|s| s.offset == slot.offset || s.name == slot.name)
            || slot
                .signature
                .as_ref()
                .is_some_and(|s| s.arguments.len() > 32)
        {
            return Err(invalid(
                "interface slots must have unique names and aligned nonoverlapping layout ranges",
            ));
        }
        if let Some(signature) = &slot.signature
            && (signature.variadic
                || !value_type(&signature.result, true)
                || signature
                    .arguments
                    .iter()
                    .any(|a| !value_type(&a.value_type, false)))
        {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported interface signature; integer/pointer nonvariadic profile required",
            ));
        }
    }
    for (i, guard) in d.guards.iter().enumerate() {
        if d.guards[..i].contains(guard) {
            return Err(invalid("duplicate interface guard"));
        }
        if let InterfaceGuard::RuntimeValue {
            offset,
            width,
            mask,
            value,
            purpose,
        } = guard
        {
            let bits = match width {
                1 => 255,
                2 => 65535,
                4 => u32::MAX,
                _ => return Err(invalid("interface guard width must be 1, 2 or 4")),
            };
            if !offset.is_multiple_of(u32::from(*width))
                || offset
                    .checked_add(u32::from(*width))
                    .is_none_or(|end| end > d.layout_bytes)
                || *mask == 0
                || mask & !bits != 0
                || value & !mask != 0
                || !text(purpose)
            {
                return Err(invalid("interface guard range, mask or value is invalid"));
            }
            // Compare overlapping bytes, including differently sized guards.
            for other in &d.guards[..i] {
                if let InterfaceGuard::RuntimeValue {
                    offset: at,
                    width: w,
                    mask: m,
                    value: v,
                    ..
                } = other
                {
                    for byte in 0..u32::from(*width) {
                        let address = offset + byte;
                        if address >= *at && address < at + u32::from(*w) {
                            let a = (mask >> (byte * 8)) & 255;
                            let b = (m >> ((address - at) * 8)) & 255;
                            if ((value >> (byte * 8)) ^ (v >> ((address - at) * 8))) & a & b != 0 {
                                return Err(invalid("contradictory interface runtime guards"));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Exact same paths, or provably overlapping static offsets from one root.
pub(super) fn overlap(a: &InterfaceContract, b: &InterfaceContract) -> bool {
    if a.root == b.root && a.path == b.path {
        return true;
    }
    let roots_match = match (&a.root, &b.root) {
        (InterfaceRoot::Address { .. }, InterfaceRoot::Address { .. }) => true,
        (InterfaceRoot::Section { section: a, .. }, InterfaceRoot::Section { section: b, .. }) => {
            a == b
        }
        (InterfaceRoot::Symbol { symbol: a, .. }, InterfaceRoot::Symbol { symbol: b, .. }) => {
            a == b
        }
        (
            InterfaceRoot::FunctionArgument {
                function: a,
                argument: x,
            },
            InterfaceRoot::FunctionArgument {
                function: b,
                argument: y,
            },
        ) => a == b && x == y,
        _ => false,
    };
    let range = |d: &InterfaceContract| {
        let mut at = match &d.root {
            InterfaceRoot::Address { address } => i128::from(*address),
            InterfaceRoot::Symbol { addend, .. } => i128::from(*addend),
            InterfaceRoot::Section { offset, .. } => i128::from(*offset),
            _ => 0,
        };
        for step in &d.path {
            if let InterfaceStep::Offset { bytes } = step {
                at += i128::from(*bytes);
            } else {
                return None;
            }
        }
        Some((at, at + i128::from(d.layout_bytes)))
    };
    roots_match && matches!((range(a),range(b)),(Some((a,x)),Some((b,y))) if a < y && b < x)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn proposal() -> KnowledgeProposal {
        let payload = ArtifactId::of_bytes(b"fixture");
        KnowledgeProposal {
            subject: "fixture.interface".to_owned().try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: payload.as_str().parse().unwrap(),
                source: FunctionSource::Input { input: 0 },
                object: ObjectId {
                    artifact: payload.clone(),
                    location: ObjectLocation::Standalone,
                },
                symbol: None,
            },
            claim: KnowledgeClaim::Interface {
                contract: Box::new(InterfaceContract {
                    root: InterfaceRoot::Address { address: 0x1000 },
                    path: vec![],
                    layout_version: "fixture/1".into(),
                    layout_bytes: 16,
                    pointer_bytes: 4,
                    abi: CallAbi::RiscvInteger,
                    index_domains: vec![],
                    guards: vec![],
                    slots: vec![InterfaceSlot {
                        offset: 4,
                        name: "callback".into(),
                        semantic: Some("fixture.callback".to_owned().try_into().unwrap()),
                        signature: Some(InterfaceSignature {
                            arguments: vec![],
                            result: InterfaceValueType::Integer {
                                bits: 32,
                                signed: false,
                            },
                            variadic: false,
                        }),
                    }],
                    purpose: "fixture".into(),
                    applicability: "selected occurrence only".into(),
                }),
            },
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
    fn contract(p: &mut KnowledgeProposal) -> &mut InterfaceContract {
        let KnowledgeClaim::Interface { contract } = &mut p.claim else {
            panic!()
        };
        contract
    }
    #[test]
    fn guards_domains_and_abi_fail_closed_before_review() {
        let p = proposal();
        validate_proposal(&p).unwrap();
        for change in 0..11 {
            let mut bad = p.clone();
            let d = contract(&mut bad);
            match change {
                0 => d.pointer_bytes = 8,
                1 => d.slots[0].signature.as_mut().unwrap().variadic = true,
                2 => {
                    d.slots[0].signature.as_mut().unwrap().result = InterfaceValueType::Integer {
                        bits: 128,
                        signed: false,
                    }
                }
                3 => d.slots.push(d.slots[0].clone()),
                4 => d.guards.push(InterfaceGuard::RuntimeValue {
                    offset: 16,
                    width: 4,
                    mask: 1,
                    value: 0,
                    purpose: "outside".into(),
                }),
                5 => d.guards.push(InterfaceGuard::RuntimeValue {
                    offset: 0,
                    width: 1,
                    mask: 256,
                    value: 0,
                    purpose: "wide mask".into(),
                }),
                6 => d.guards.push(InterfaceGuard::RuntimeValue {
                    offset: 0,
                    width: 1,
                    mask: 1,
                    value: 2,
                    purpose: "unmasked value".into(),
                }),
                7 => d.path.push(InterfaceStep::Index {
                    argument: 1,
                    stride: 4,
                }),
                8 => {
                    d.path.push(InterfaceStep::Index {
                        argument: 1,
                        stride: 4,
                    });
                    d.index_domains.push(InterfaceIndexDomain {
                        argument: 1,
                        min: 4,
                        max: 3,
                        reason: "empty".into(),
                    });
                }
                9 => {
                    d.path.push(InterfaceStep::Index {
                        argument: 1,
                        stride: 4,
                    });
                    d.index_domains.push(InterfaceIndexDomain {
                        argument: 1,
                        min: 0,
                        max: u32::MAX,
                        reason: "overflow".into(),
                    });
                }
                _ => {
                    d.root = InterfaceRoot::Address {
                        address: u32::MAX - 3,
                    }
                }
            }
            assert!(validate_proposal(&bad).is_err(), "case {change}");
        }
        let mut valid = p;
        let d = contract(&mut valid);
        d.path.push(InterfaceStep::Index {
            argument: 1,
            stride: 4,
        });
        d.index_domains.push(InterfaceIndexDomain {
            argument: 1,
            min: 0,
            max: 7,
            reason: "declared caller precondition".into(),
        });
        d.guards.push(InterfaceGuard::RuntimeValue {
            offset: 0,
            width: 4,
            mask: 0xff00,
            value: 0x1200,
            purpose: "tag".into(),
        });
        d.guards.push(InterfaceGuard::RuntimeValue {
            offset: 1,
            width: 1,
            mask: 255,
            value: 0x12,
            purpose: "same tag byte".into(),
        });
        validate_proposal(&valid).unwrap();
        let d = contract(&mut valid);
        let InterfaceGuard::RuntimeValue { value, .. } = &mut d.guards[1] else {
            panic!()
        };
        *value = 0x13;
        assert!(validate_proposal(&valid).is_err());
    }
    #[test]
    fn shifted_layout_conflicts_preserve_physical_address_and_path_identity() {
        let a = proposal();
        let mut b = a.clone();
        b.subject = "another.interface".to_owned().try_into().unwrap();
        contract(&mut b).root = InterfaceRoot::Address { address: 0x100c };
        assert!(conflicts(&a, &b));
        contract(&mut b).root = InterfaceRoot::Address { address: 0x1010 };
        assert!(!conflicts(&a, &b));
        contract(&mut b).root = InterfaceRoot::Address { address: 0x1000 };
        contract(&mut b)
            .path
            .push(InterfaceStep::Offset { bytes: 4 });
        assert!(conflicts(&a, &b));
        b.occurrence.source = FunctionSource::Input { input: 1 };
        assert!(!conflicts(&a, &b));
    }
}
