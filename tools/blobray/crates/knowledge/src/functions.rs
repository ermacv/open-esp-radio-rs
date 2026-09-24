//! Pure bounded function/context admission; preconditions are never runtime observations.
use super::*;
fn text(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control)
}
fn max_value(width: u8) -> u64 {
    u64::MAX >> (64 - 8 * width)
}
fn argument_width(d: &FunctionContract, argument: u8) -> Result<u8> {
    d.signature
        .as_ref()
        .and_then(|s| s.arguments.get(argument as usize))
        .and_then(|a| calls::width(&a.value_type))
        .ok_or_else(|| {
            invalid("argument predicate requires an explicit supported signature argument")
        })
}
fn in_context(c: &ArgumentContext, offset: i32, width: u8) -> bool {
    i64::from(offset) >= i64::from(c.start)
        && i64::from(offset) + i64::from(width) <= i64::from(c.start) + i64::from(c.length)
}
// Four tight-bound states, 64 bit positions. No enumeration of the value domain.
fn satisfiable(low: u64, high: u64, mask: u64, value: u64) -> bool {
    if low > high {
        return false;
    }
    let mut states = [false; 4];
    states[3] = true;
    for bit in (0..64).rev() {
        let lo = (low >> bit) & 1;
        let hi = (high >> bit) & 1;
        let mut next = [false; 4];
        for (state, active) in states.iter().enumerate() {
            if !active {
                continue;
            }
            for digit in 0..=1 {
                if (mask >> bit) & 1 != 0 && digit != (value >> bit) & 1 {
                    continue;
                }
                let tight_low = state & 1 != 0;
                let tight_high = state & 2 != 0;
                if tight_low && digit < lo || tight_high && digit > hi {
                    continue;
                }
                let state = usize::from(tight_low && digit == lo)
                    | (usize::from(tight_high && digit == hi) << 1);
                next[state] = true;
            }
        }
        states = next;
    }
    states.into_iter().any(|x| x)
}
pub(super) fn validate(p: &KnowledgeProposal, d: &FunctionContract) -> Result<()> {
    if d.selector.object() != &p.occurrence.object
        || p.occurrence
            .symbol
            .as_ref()
            .is_some_and(|s| d.selector.symbol() != Some(s))
        || !text(&d.name)
        || !text(&d.summary)
        || !text(&d.applicability)
        || d.name.len() > 256
        || d.contexts.len() > 16
        || d.preconditions.len() > 64
    {
        return Err(invalid(
            "function contract requires exact occurrence and bounded names, contexts and predicates",
        ));
    }
    if d.abi != CallAbi::RiscvInteger {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported function contract ABI",
        ));
    }
    if let Some(signature) = &d.signature {
        calls::validate(signature)?;
    }
    if d.return_role.is_some()
        && d.signature
            .as_ref()
            .is_none_or(|s| matches!(s.result, AbiValueType::Void))
    {
        return Err(invalid(
            "return role requires an explicit non-void return type",
        ));
    }
    let mut fields = 0usize;
    for (i, context) in d.contexts.iter().enumerate() {
        fields = fields.saturating_add(context.fields.len());
        if context.argument >= 32
            || !text(&context.name)
            || context.length == 0
            || fields > 128
            || i64::from(context.start) + i64::from(context.length) > i64::from(i32::MAX) + 1
            || d.contexts[..i]
                .iter()
                .any(|c| c.argument == context.argument || c.name == context.name)
            || d.signature.as_ref().is_some_and(|s| {
                s.arguments
                    .get(context.argument as usize)
                    .is_none_or(|a| !matches!(a.value_type, AbiValueType::Pointer { .. }))
            })
        {
            return Err(invalid(
                "invalid argument context, duplicate root, or non-pointer signature argument",
            ));
        }
        for (j, field) in context.fields.iter().enumerate() {
            if !text(&field.name)
                || !matches!(field.width, 1 | 2 | 4 | 8)
                || !in_context(context, field.offset, field.width)
                || field
                    .value_type
                    .as_ref()
                    .is_some_and(|t| calls::width(t) != Some(field.width))
                || context.fields[..j].iter().any(|f| {
                    f.name == field.name
                        || (i64::from(f.offset) < i64::from(field.offset) + i64::from(field.width)
                            && i64::from(field.offset) < i64::from(f.offset) + i64::from(f.width))
                })
            {
                return Err(invalid(
                    "context fields need unique names, nonoverlapping bounded extents and matching types",
                ));
            }
        }
    }
    let mut lows = [0u64; 32];
    let mut highs = [u64::MAX; 32];
    if let Some(signature) = &d.signature {
        for (i, argument) in signature.arguments.iter().enumerate() {
            highs[i] = max_value(calls::width(&argument.value_type).expect("signature validated"));
            if matches!(
                argument.value_type,
                AbiValueType::Pointer { nullable: false }
            ) {
                lows[i] = 1;
            }
        }
    }
    let mut masks = [0u64; 32];
    let mut values = [0u64; 32];
    for (i, predicate) in d.preconditions.iter().enumerate() {
        match predicate {
            FunctionPrecondition::ArgumentRange { argument, min, max, reason } => {
                let width = argument_width(d, *argument)?;
                if !text(reason) || min > max || *max > max_value(width) {
                    return Err(invalid("invalid raw-bit-pattern argument range"));
                }
                let a = *argument as usize;
                lows[a] = lows[a].max(*min);
                highs[a] = highs[a].min(*max);
            }
            FunctionPrecondition::ArgumentBits { argument, mask, value, reason } => {
                let width = argument_width(d, *argument)?;
                if !text(reason) || *mask == 0 || *mask > max_value(width) || value & !mask != 0 {
                    return Err(invalid("invalid argument mask/value"));
                }
                let a = *argument as usize;
                if (values[a] ^ value) & masks[a] & mask != 0 {
                    return Err(invalid("contradictory argument bit predicates"));
                }
                masks[a] |= mask;
                values[a] |= value;
                highs[a] = highs[a].min(max_value(width));
            }
            FunctionPrecondition::ContextBits { argument, offset, width, mask, value, reason } => {
                let context = d.contexts.iter().find(|c| c.argument == *argument)
                    .ok_or_else(|| invalid("context predicate requires an explicit context extent"))?;
                if !text(reason) || !matches!(width, 1 | 2 | 4 | 8) || !in_context(context, *offset, *width)
                    || *mask == 0 || *mask > max_value(*width) || value & !mask != 0 {
                    return Err(invalid("invalid context byte predicate"));
                }
                for prior in &d.preconditions[..i] {
                    let FunctionPrecondition::ContextBits { argument: a, offset: o, width: w, mask: m, value: v, .. } = prior else { continue; };
                    if a != argument { continue; }
                    for byte in i64::from(*offset).max(i64::from(*o))..(i64::from(*offset) + i64::from(*width)).min(i64::from(*o) + i64::from(*w)) {
                        let shift = 8 * (byte - i64::from(*offset));
                        let other = 8 * (byte - i64::from(*o));
                        if ((value >> shift) ^ (v >> other)) & (mask >> shift) & (m >> other) & 255 != 0 {
                            return Err(invalid("contradictory overlapping context byte predicates"));
                        }
                    }
                }
            }
            FunctionPrecondition::Assumption { id, statement, reason } => {
                if !text(statement) || !text(reason)
                    || d.preconditions[..i].iter().any(|p| matches!(p,FunctionPrecondition::Assumption { id: other, .. } if other == id)) {
                    return Err(invalid("named assumptions require unique identities and attributed text"));
                }
            }
        }
    }
    if (0..32).any(|a| !satisfiable(lows[a], highs[a], masks[a], values[a])) {
        return Err(invalid(
            "argument range and mask preconditions have no common value",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn proposal() -> KnowledgeProposal {
        let payload = ArtifactId::of_bytes(b"function contract");
        let object = ObjectId {
            artifact: payload.clone(),
            location: ObjectLocation::Standalone,
        };
        KnowledgeProposal {
            subject: "fixture.function".to_owned().try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: payload.as_str().parse().unwrap(),
                source: FunctionSource::Input { input: 0 },
                object: object.clone(),
                symbol: None,
            },
            claim: KnowledgeClaim::Function {
                contract: Box::new(FunctionContract {
                    selector: FunctionSelector::Range {
                        object,
                        section: 1,
                        extent: CodeRange {
                            start: 0,
                            length: 8,
                        },
                    },
                    abi: CallAbi::RiscvInteger,
                    signature: Some(CallSignature {
                        arguments: vec![
                            CallArgument {
                                role: "fixture.context".to_owned().try_into().unwrap(),
                                value_type: AbiValueType::Pointer { nullable: false },
                            },
                            CallArgument {
                                role: "fixture.mode".to_owned().try_into().unwrap(),
                                value_type: AbiValueType::Integer {
                                    bits: 8,
                                    signed: false,
                                },
                            },
                        ],
                        result: AbiValueType::Integer {
                            bits: 32,
                            signed: true,
                        },
                        variadic: false,
                    }),
                    name: "fixture".into(),
                    role: None,
                    return_role: Some("fixture.status".to_owned().try_into().unwrap()),
                    summary: "conditional declaration".into(),
                    applicability: "fixture only".into(),
                    contexts: vec![ArgumentContext {
                        argument: 0,
                        name: "context".into(),
                        start: -4,
                        length: 16,
                        fields: vec![ContextField {
                            offset: -4,
                            width: 4,
                            name: "tag".into(),
                            value_type: Some(AbiValueType::Integer {
                                bits: 32,
                                signed: false,
                            }),
                            access: ContextAccess::Read,
                            role: None,
                        }],
                    }],
                    preconditions: vec![
                        FunctionPrecondition::ArgumentRange {
                            argument: 1,
                            min: 2,
                            max: 8,
                            reason: "finite mode".into(),
                        },
                        FunctionPrecondition::ArgumentBits {
                            argument: 1,
                            mask: 1,
                            value: 1,
                            reason: "odd mode".into(),
                        },
                        FunctionPrecondition::ContextBits {
                            argument: 0,
                            offset: -4,
                            width: 4,
                            mask: 0xff00,
                            value: 0x1200,
                            reason: "required tag".into(),
                        },
                    ],
                }),
            },
            evidence: vec![EvidenceRef::Source {
                payload,
                range: CodeRange {
                    start: 0,
                    length: 8,
                },
            }],
            note: None,
        }
    }
    fn contract(p: &mut KnowledgeProposal) -> &mut FunctionContract {
        let KnowledgeClaim::Function { contract } = &mut p.claim else {
            panic!()
        };
        contract
    }
    #[test]
    fn context_layout_and_cross_predicate_contradictions_fail_before_review() {
        let p = proposal();
        validate_proposal(&p).unwrap();
        let mut reversed = p.clone();
        contract(&mut reversed).preconditions.reverse();
        validate_proposal(&reversed).unwrap();
        for case in 0..16 {
            let mut bad = p.clone();
            let c = contract(&mut bad);
            match case {
                0 => c.contexts[0].fields[0].offset = -5,
                1 => c.contexts[0].fields[0].width = 8,
                2 => {
                    let duplicate = c.contexts[0].fields[0].clone();
                    c.contexts[0].fields.push(duplicate);
                }
                3 => c.contexts[0].argument = 1,
                4 => c.contexts[0].length = 0,
                5 => c.signature.as_mut().unwrap().variadic = true,
                6 => c.signature.as_mut().unwrap().result = AbiValueType::Void,
                7 => c.preconditions.push(FunctionPrecondition::ArgumentRange {
                    argument: 1,
                    min: 2,
                    max: 2,
                    reason: "even contradicts odd".into(),
                }),
                8 => c.preconditions.push(FunctionPrecondition::ArgumentBits {
                    argument: 1,
                    mask: 1,
                    value: 0,
                    reason: "opposite bit".into(),
                }),
                9 => c.preconditions.push(FunctionPrecondition::ContextBits {
                    argument: 0,
                    offset: -3,
                    width: 1,
                    mask: 255,
                    value: 0x13,
                    reason: "overlapping byte contradicts word".into(),
                }),
                10 => c.preconditions.push(FunctionPrecondition::ArgumentRange {
                    argument: 1,
                    min: 0,
                    max: 256,
                    reason: "outside ABI width".into(),
                }),
                11 => {
                    c.signature = None;
                    c.return_role = None;
                }
                12 => c.contexts[0].fields[0].value_type = Some(AbiValueType::Void),
                13 => c.contexts[0].start = i32::MAX,
                14 => c.preconditions.push(FunctionPrecondition::ArgumentRange {
                    argument: 0,
                    min: 0,
                    max: 0,
                    reason: "null contradicts nonnull pointer".into(),
                }),
                _ => c.preconditions.push(FunctionPrecondition::ContextBits {
                    argument: 0,
                    offset: 11,
                    width: 4,
                    mask: 1,
                    value: 1,
                    reason: "outside context".into(),
                }),
            }
            assert!(validate_proposal(&bad).is_err(), "case {case}");
        }
        let mut unknown = p.clone();
        let c = contract(&mut unknown);
        c.signature = None;
        c.return_role = None;
        c.preconditions.clear();
        c.contexts[0].fields[0].value_type = None;
        validate_proposal(&unknown).unwrap();
        assert!(conflicts(&p, &unknown));
        unknown.occurrence.source = FunctionSource::Input { input: 1 };
        assert!(!conflicts(&p, &unknown));
    }
    #[test]
    fn masked_range_solver_matches_independent_small_domain_enumeration_and_u64_edges() {
        let mut seed = 7u64;
        for _ in 0..4096 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let low = seed & 255;
            let high = (seed >> 8) & 255;
            let mask = (seed >> 16) & 255;
            let value = (seed >> 24) & mask;
            let expected = (0..=255).any(|x| x >= low && x <= high && x & mask == value);
            assert_eq!(satisfiable(low, high, mask, value), expected);
        }
        assert!(satisfiable(u64::MAX, u64::MAX, u64::MAX, u64::MAX));
        assert!(!satisfiable(0, (1 << 63) - 1, 1 << 63, 1 << 63));
        assert!(satisfiable(1 << 63, u64::MAX, 1 << 63, 1 << 63));
    }
}
