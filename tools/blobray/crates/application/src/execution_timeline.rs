//! Guest memory capture uses instruction identity, never progress metadata or inspection reads.
use super::*;
impl Session<'_> {
    pub(super) fn admit_memory_event(&self, selected: bool, c: &mut dyn RunControl) -> Result<()> {
        if !selected {
            return Ok(());
        }
        c.checkpoint(1)?;
        if self.pc.is_none() {
            return Err(Error::new(
                ErrorCode::Integrity,
                "memory observation lacks instruction owner",
            ));
        }
        if self.events.len() == self.max_events {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "normal-memory event capacity exhausted",
            ));
        }
        Ok(())
    }
    pub(super) fn trace_memory(
        &mut self,
        transaction: MemoryTransaction,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if !self.timeline.selects(&transaction) {
            return Ok(());
        }
        self.admit_memory_event(true, c)?;
        self.event(
            ExecutionEvent::Memory {
                site: self.pc.unwrap(),
                transaction,
            },
            c,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trace_admission_precedes_atomic_callback_and_unreadable_memory_stays_explicit() {
        let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
        let mut c = || Ok(());
        let mut s = Session::new(&memory, 1, &mut c).unwrap();
        s.region(
            Mapping {
                address: 0x3000,
                length: 4,
                flags: 6,
                kind: RegionKind::Ram(RegionLifetime::Session),
            },
            Some(0),
            &[],
            &mut c,
        )
        .unwrap();
        s.instruction(0x1000);
        s.timeline = TimelineCapture {
            reads: true,
            atomics: true,
            ..Default::default()
        };
        assert_eq!(
            s.read(0x3000, 4, MemoryAccess::Read, &mut c).unwrap(),
            Some(0)
        );
        let mut called = false;
        assert_eq!(
            s.modify_word(
                0x3000,
                ExecutionOrdering {
                    acquire: false,
                    release: false
                },
                &mut |_| {
                    called = true;
                    9
                },
                &mut c
            )
            .unwrap_err()
            .code,
            ErrorCode::ResourceLimited
        );
        assert!(!called);
        assert_eq!(&s.regions[0].bytes[..], &[0; 4]);
        s.events.clear();
        s.regions[0].flags = 2;
        assert_eq!(s.read(0x3000, 4, MemoryAccess::Read, &mut c).unwrap(), None);
        assert!(matches!(
            s.events[0],
            ExecutionEvent::Memory {
                site: 0x1000,
                transaction: MemoryTransaction::Read {
                    value: MemoryReadValue::Unavailable,
                    ..
                }
            }
        ));
        s.finish_phase();
        assert!(s.pc.is_none());
        assert!(!s.timeline.any());
    }
}
