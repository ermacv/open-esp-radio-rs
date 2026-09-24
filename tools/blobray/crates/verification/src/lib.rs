//! Comparison of explicitly selected concrete observations; no execution/store authority.
use blobray_domain::*;
pub const VERIFIER: &str = "selected-events-calls-returns-memory/model-6";
pub fn compare(
    left: &ExecutionObservation,
    right: &ExecutionObservation,
    relation: &ComparisonRelation,
    control: &mut dyn RunControl,
) -> Result<CaseComparison> {
    let different = |difference| CaseComparison {
        verdict: ComparisonVerdict::Diff,
        difference: Some(difference),
    };
    let mut known = true;
    control.checkpoint((left.events.len() + right.events.len()) as u64)?;
    let mut le = Selected::new(&left.events, relation);
    let mut re = Selected::new(&right.events, relation);
    let mut count = 0;
    loop {
        let (a, b) = (le.next()?, re.next()?);
        match (a, b) {
            (Some(a), Some(b)) => {
                control.checkpoint(1)?;
                match (a, b) {
                    (
                        Observation::Call {
                            target: a,
                            arguments: aa,
                        },
                        Observation::Call {
                            target: b,
                            arguments: ba,
                        },
                    ) => {
                        if a != b {
                            return Ok(different(ComparisonDifference::CallTarget {
                                index: count,
                                vendor: a,
                                replacement: b,
                            }));
                        }
                        if aa.len() != ba.len() {
                            return Err(Error::new(
                                ErrorCode::Integrity,
                                "same physical target has different capture widths",
                            ));
                        }
                        for (word, (a, b)) in aa.iter().zip(ba).enumerate() {
                            control.checkpoint(1)?;
                            let value = |e: &ExecutionEvent| match e {
                                ExecutionEvent::TransferArgument { value, .. } => value.value(),
                                _ => unreachable!(),
                            };
                            match (value(a), value(b)) {
                                (Some(a), Some(b)) if a != b => {
                                    return Ok(different(ComparisonDifference::CallArgument {
                                        index: count,
                                        word: word as u16,
                                        vendor: a,
                                        replacement: b,
                                    }));
                                }
                                (Some(_), Some(_)) => {}
                                _ => known = false,
                            }
                        }
                    }
                    (Observation::Event(a), Observation::Event(b)) if a == b => {}
                    _ => return Ok(different(ComparisonDifference::Event { index: count })),
                }
                count += 1;
            }
            (None, None) => break,
            (None, Some(_)) if left.stop.completed() => {
                return Ok(different(ComparisonDifference::Event { index: count }));
            }
            (Some(_), None) if right.stop.completed() => {
                return Ok(different(ComparisonDifference::Event { index: count }));
            }
            _ => {
                known = false;
                break;
            }
        }
    }
    for (word, selected) in [relation.returns.low, relation.returns.high]
        .into_iter()
        .enumerate()
    {
        if !selected {
            continue;
        }
        control.checkpoint(1)?;
        let values = |o: &ExecutionObservation| match &o.stop {
            ExecutionStop::Returned { low, high } => [*low, *high][word],
            _ => None,
        };
        match (values(left), values(right)) {
            (Some(a), Some(b)) if a != b => {
                return Ok(different(ComparisonDifference::Return {
                    word: word as u8,
                    vendor: a,
                    replacement: b,
                }));
            }
            (Some(_), Some(_)) => {}
            _ => known = false,
        }
    }
    fn selected(chunks: &[FinalMemoryChunk], selection: u16) -> &[FinalMemoryChunk] {
        let start = chunks.partition_point(|c| c.selection < selection);
        let end = chunks.partition_point(|c| c.selection <= selection);
        &chunks[start..end]
    }
    for (pair, p) in relation.memory.iter().enumerate() {
        control.checkpoint(
            2 * (left.final_memory.len().max(1).ilog2() as u64
                + right.final_memory.len().max(1).ilog2() as u64
                + 2),
        )?;
        let a = selected(&left.final_memory, p.vendor);
        let b = selected(&right.final_memory, p.replacement);
        if a.is_empty() || b.is_empty() || a.len() != b.len() {
            known = false;
            continue;
        }
        // A stopped intermediate RAM state cannot prove a difference in completed final states.
        let at_goals = left.stop.completed() && right.stop.completed();
        known &= at_goals;
        for (a, b) in a.iter().zip(b) {
            control.checkpoint(MEMORY_CHUNK_BYTES as u64)?;
            a.validate()?;
            b.validate()?;
            if a.offset != b.offset || a.length != b.length {
                known = false;
                continue;
            }
            for i in 0..a.length as usize {
                if a.known & b.known & (1 << i) == 0 {
                    known = false;
                    continue;
                }
                if at_goals && a.bytes[i] != b.bytes[i] {
                    return Ok(different(ComparisonDifference::Memory {
                        pair: pair as u16,
                        offset: a.offset + i as u32,
                        vendor: a.bytes[i],
                        replacement: b.bytes[i],
                    }));
                }
            }
        }
    }
    Ok(CaseComparison {
        verdict: if known
            && left.completed()
            && right.completed()
            && std::mem::discriminant(&left.stop) == std::mem::discriminant(&right.stop)
        {
            ComparisonVerdict::Match
        } else {
            ComparisonVerdict::Incomplete
        },
        difference: None,
    })
}
enum Observation<'a> {
    Event(&'a ExecutionEvent),
    Call {
        target: u32,
        arguments: &'a [ExecutionEvent],
    },
}
struct Selected<'a> {
    remaining: &'a [ExecutionEvent],
    relation: &'a ComparisonRelation,
}
impl<'a> Selected<'a> {
    fn new(remaining: &'a [ExecutionEvent], relation: &'a ComparisonRelation) -> Self {
        Self {
            remaining,
            relation,
        }
    }
    fn next(&mut self) -> Result<Option<Observation<'a>>> {
        let invalid = || {
            Error::new(
                ErrorCode::Integrity,
                "invalid physical call observation group",
            )
        };
        while let Some((event, rest)) = self.remaining.split_first() {
            self.remaining = rest;
            if let ExecutionEvent::CallTransfer { target, words, .. } = event {
                if usize::from(*words) > MAX_EXECUTION_ARGUMENT_WORDS
                    || rest.len() < usize::from(*words)
                {
                    return Err(invalid());
                }
                let (arguments, rest) = rest.split_at(usize::from(*words));
                if !arguments.iter().enumerate().all(|(i, e)|
                    matches!(e, ExecutionEvent::TransferArgument { word, .. } if usize::from(*word) == i)) {
                    return Err(invalid());
                }
                self.remaining = rest;
                if self.relation.calls {
                    return Ok(Some(Observation::Call {
                        target: *target,
                        arguments,
                    }));
                }
            } else if matches!(event, ExecutionEvent::TransferArgument { .. }) {
                return Err(invalid());
            } else if self.relation.events.selects(event) {
                return Ok(Some(Observation::Event(event)));
            }
        }
        Ok(None)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn compare(
        left: &ExecutionObservation,
        right: &ExecutionObservation,
        compare_return: bool,
        c: &mut dyn RunControl,
    ) -> Result<CaseComparison> {
        super::compare(
            left,
            right,
            &ComparisonRelation {
                calls: false,
                returns: ReturnWords {
                    low: compare_return,
                    high: false,
                },
                events: EventChannels {
                    mmio_read: true,
                    mmio_write: true,
                    fence: true,
                    delay: true,
                },
                memory: vec![],
            },
            c,
        )
    }
    fn observed(stop: ExecutionStop, events: Vec<ExecutionEvent>) -> ExecutionObservation {
        ExecutionObservation {
            stop,
            steps: 1,
            events,
            models: vec![],
            calls: vec![],
            tables: vec![],
            services: vec![],
            final_memory: vec![],
        }
    }
    #[test]
    fn verdicts_respect_unknown_returns_and_incomplete_prefixes() {
        let done = observed(
            ExecutionStop::Returned {
                low: Some(7),
                high: None,
            },
            vec![],
        );
        let missing = observed(ExecutionStop::BlockedByPriorPhase, vec![]);
        assert_eq!(
            compare(&done, &done, true, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Match
        );
        assert_eq!(
            compare(&done, &missing, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let more = observed(
            ExecutionStop::BlockedByPriorPhase,
            vec![ExecutionEvent::Write {
                address: 16,
                width: 4,
                value: 1,
            }],
        );
        assert_eq!(
            compare(&done, &more, true, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Diff
        );
        assert_eq!(
            compare(&missing, &more, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let unknown = observed(
            ExecutionStop::Returned {
                low: None,
                high: None,
            },
            vec![],
        );
        assert_eq!(
            compare(&unknown, &done, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
    }
}

#[cfg(test)]
mod physical_calls {
    use super::*;
    fn observation(targets: &[u32], word: ObservedWord, complete: bool) -> ExecutionObservation {
        let events = targets
            .iter()
            .flat_map(|target| {
                [
                    ExecutionEvent::CallTransfer {
                        site: 0x1000,
                        target: *target,
                        tail: false,
                        indirect: false,
                        stack: Some(0x9000),
                        target_kind: ObservedCallTarget::CapturedCode,
                        words: 1,
                    },
                    ExecutionEvent::TransferArgument {
                        word: 0,
                        value: word,
                    },
                ]
            })
            .collect();
        ExecutionObservation {
            stop: if complete {
                ExecutionStop::Returned {
                    low: Some(0),
                    high: None,
                }
            } else {
                ExecutionStop::BlockedByPriorPhase
            },
            steps: 1,
            events,
            final_memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![],
            services: vec![],
        }
    }
    #[test]
    fn reordered_missing_and_unknown_call_prefixes_fail_closed() {
        let r = ComparisonRelation {
            returns: ReturnWords {
                low: false,
                high: false,
            },
            events: EventChannels {
                mmio_read: true,
                mmio_write: true,
                fence: true,
                delay: true,
            },
            memory: vec![],
            calls: true,
        };
        let known = ObservedWord::Known { value: 7 };
        let a = observation(&[0x2000, 0x3000], known, true);
        for b in [
            observation(&[0x3000, 0x2000], known, true),
            observation(&[0x2000], known, true),
        ] {
            assert_eq!(
                compare(&a, &b, &r, &mut || Ok(())).unwrap().verdict,
                ComparisonVerdict::Diff
            );
        }
        let b = observation(&[0x2000], known, false);
        assert_eq!(
            compare(&a, &b, &r, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Incomplete
        );
        let b = observation(&[0x2000, 0x3000], ObservedWord::Unknown, true);
        assert_eq!(
            compare(&a, &b, &r, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Incomplete
        );
        let mut b = observation(&[0x2000, 0x4000], ObservedWord::Unknown, true);
        assert_eq!(
            compare(&a, &b, &r, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Diff
        );
        b.events.truncate(1);
        assert_eq!(
            compare(&a, &b, &r, &mut || Ok(())).unwrap_err().code,
            ErrorCode::Integrity
        );
    }
}
