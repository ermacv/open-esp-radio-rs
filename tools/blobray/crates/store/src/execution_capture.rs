//! Bounded structural validation of physical call groups; no instruction re-execution.
use super::*;
pub(super) struct CaptureState {
    next: u16,
    words: u16,
    stack: Option<u32>,
    last: Option<(u32, u32, bool)>,
    pub known: bool,
}
impl CaptureState {
    pub fn new() -> Self {
        Self {
            next: 0,
            words: 0,
            stack: None,
            last: None,
            known: true,
        }
    }
    pub fn finish(&self, input: &Invocation, stop: &ExecutionStop) -> Result<()> {
        if self.next != self.words {
            return Err(integrity("missing physical call arguments"));
        }
        if let ExecutionStop::ObservedCall { pc, target, tail } = stop
            && input
                .observe_calls
                .as_ref()
                .is_some_and(|p| !tail || p.include_tail)
            && self.last != Some((*pc, *target, *tail))
        {
            return Err(integrity("observe-call outcome lacks captured boundary"));
        }
        Ok(())
    }
    pub fn event(&mut self, input: &Invocation, event: &ExecutionEvent) -> Result<()> {
        if let ExecutionEvent::TransferArgument { word, value } = event {
            if *word != self.next || self.next == self.words {
                return Err(integrity("physical call argument lacks ordered boundary"));
            }
            if let ObservedWord::Unavailable { reason } = value {
                let valid = *word >= 8
                    && match reason {
                        WordAccess::UnknownStack => self.stack.is_none(),
                        WordAccess::MisalignedStack => {
                            self.stack.is_some_and(|s| !s.is_multiple_of(16))
                        }
                        WordAccess::OutsidePrivateStack => {
                            self.stack.is_some_and(|s| s.is_multiple_of(16))
                        }
                    };
                if !valid {
                    return Err(integrity(
                        "physical call argument unavailable reason differs",
                    ));
                }
            } else if *word >= 8 && self.stack.is_none_or(|s| !s.is_multiple_of(16)) {
                return Err(integrity("physical stack argument has no aligned stack"));
            }
            self.known &= value.value().is_some();
            self.next += 1;
            return Ok(());
        }
        if self.next != self.words {
            return Err(integrity("event interrupts physical call arguments"));
        }
        if let ExecutionEvent::CallTransfer {
            site,
            target,
            tail,
            stack,
            words,
            ..
        } = event
        {
            let p = input
                .observe_calls
                .as_ref()
                .ok_or_else(|| integrity("unrequested physical call capture"))?;
            let expected = p
                .overrides
                .iter()
                .find(|o| o.target == *target)
                .map(|o| o.words)
                .unwrap_or(p.argument_words);
            if site & 1 != 0
                || *site >= u32::MAX - 1
                || target & 1 != 0
                || *target >= u32::MAX - 1
                || (*tail && !p.include_tail)
                || *words != expected
            {
                return Err(integrity(
                    "physical call boundary differs from capture profile",
                ));
            }
            self.next = 0;
            self.words = *words;
            self.stack = *stack;
            self.last = Some((*site, *target, *tail));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_validation_rejects_unrequested_incomplete_reordered_and_invented_words() {
        let mut input = Invocation {
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![],
            services: vec![],
            observe_memory: vec![],
            observe_calls: Some(CallCapture {
                include_tail: false,
                argument_words: 2,
                overrides: vec![],
            }),
        };
        let header = ExecutionEvent::CallTransfer {
            site: 0x1000,
            target: 0x2000,
            tail: false,
            indirect: true,
            stack: Some(0x9000),
            target_kind: ObservedCallTarget::Unavailable,
            words: 2,
        };
        let word = |word, value| ExecutionEvent::TransferArgument { word, value };
        let done = ExecutionStop::Returned {
            low: Some(0),
            high: None,
        };
        let mut s = CaptureState::new();
        assert!(s.event(&input, &word(0, ObservedWord::Unknown)).is_err());
        s.event(&input, &header).unwrap();
        assert!(s.finish(&input, &done).is_err());
        assert!(s.event(&input, &header).is_err());
        assert!(s.event(&input, &word(1, ObservedWord::Unknown)).is_err());
        assert!(
            s.event(
                &input,
                &word(
                    0,
                    ObservedWord::Unavailable {
                        reason: WordAccess::UnknownStack
                    }
                )
            )
            .is_err()
        );
        s.event(&input, &word(0, ObservedWord::Known { value: 7 }))
            .unwrap();
        s.event(&input, &word(1, ObservedWord::Unknown)).unwrap();
        s.finish(&input, &done).unwrap();
        assert!(!s.known);
        assert!(s.event(&input, &word(2, ObservedWord::Unknown)).is_err());
        assert!(
            s.finish(
                &input,
                &ExecutionStop::ObservedCall {
                    pc: 0x1004,
                    target: 0x2000,
                    tail: false
                }
            )
            .is_err()
        );
        s.finish(
            &input,
            &ExecutionStop::ObservedCall {
                pc: 0x1000,
                target: 0x2000,
                tail: false,
            },
        )
        .unwrap();
        for variant in 0..4 {
            let mut changed = header.clone();
            if let ExecutionEvent::CallTransfer {
                site,
                target,
                tail,
                words,
                ..
            } = &mut changed
            {
                match variant {
                    0 => *site = 0x1001,
                    1 => *target = u32::MAX - 1,
                    2 => *tail = true,
                    _ => *words = 1,
                }
            }
            assert!(s.event(&input, &changed).is_err());
        }
        input.observe_calls = None;
        assert!(s.event(&input, &header).is_err());
    }
}
