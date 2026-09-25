//! Code one side reaches over a whole execution. It outlives every session of
//! that side, so cold resets accumulate into the same executable segments.
use crate::*;

/// Executed instruction start.
const EXECUTED: u8 = 1;
/// Conditional branch observed taken.
const TAKEN: u8 = 2;
/// Conditional branch observed falling through.
const FALLTHROUGH: u8 = 4;
/// Instructions are halfword aligned; one mark byte per halfword.
const GRANULE: u32 = 2;

struct Segment<'a> {
    address: u32,
    length: u32,
    marks: ScratchBytes<'a>,
}

#[derive(Default)]
pub(crate) struct CodeCoverage<'a> {
    segments: Vec<Segment<'a>>,
    /// Segment of the most recent mark; straight-line code stays in one segment.
    last: usize,
}

impl<'a> CodeCoverage<'a> {
    /// Track one executable captured segment. A later session maps the same
    /// segments again; those keep their accumulated marks.
    pub fn map(
        &mut self,
        address: u32,
        length: usize,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let length = u32::try_from(length)
            .map_err(|_| Error::new(ErrorCode::InvalidRequest, "segment exceeds address space"))?;
        c.checkpoint(self.segments.len() as u64 + 1)?;
        if self
            .segments
            .iter()
            .any(|s| s.address == address && s.length == length)
        {
            return Ok(());
        }
        let marks = memory.bytes(length.div_ceil(GRANULE) as usize, c.position())?;
        self.segments
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "coverage allocation refused"))?;
        self.segments.push(Segment {
            address,
            length,
            marks,
        });
        Ok(())
    }

    fn mark(&mut self, pc: u32, flag: u8) {
        let within = |s: &Segment<'_>| pc.wrapping_sub(s.address) < s.length;
        if !self.segments.get(self.last).is_some_and(within) {
            let Some(index) = self.segments.iter().position(within) else {
                return;
            };
            self.last = index;
        }
        let segment = &mut self.segments[self.last];
        segment.marks[((pc - segment.address) / GRANULE) as usize] |= flag;
    }

    pub fn instruction(&mut self, pc: u32) {
        self.mark(pc, EXECUTED);
    }

    pub fn branch(&mut self, site: u32, taken: bool) {
        self.mark(site, if taken { TAKEN } else { FALLTHROUGH });
    }

    /// The accumulated coverage, with the reservation that admits its vectors.
    pub fn finish(
        &self,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<(ExecutionCoverage, MemoryReservation<'a>)> {
        let mut segments: Vec<_> = self.segments.iter().collect();
        segments.sort_by_key(|s| s.address);
        let (mut instructions, mut branches) = (0usize, 0usize);
        for segment in &segments {
            for marks in segment.marks.chunks(WORK_BLOCK) {
                c.checkpoint(1)?;
                instructions += marks.iter().filter(|m| *m & EXECUTED != 0).count();
                branches += marks
                    .iter()
                    .filter(|m| *m & (TAKEN | FALLTHROUGH) != 0)
                    .count();
            }
        }
        let reservation = memory.reserve(
            (instructions * std::mem::size_of::<u32>()
                + branches * std::mem::size_of::<BranchCoverage>()) as u64,
            c.position(),
        )?;
        let refused = |_| Error::new(ErrorCode::ResourceLimited, "coverage allocation refused");
        let mut coverage = ExecutionCoverage::default();
        coverage
            .instructions
            .try_reserve_exact(instructions)
            .map_err(refused)?;
        coverage
            .branches
            .try_reserve_exact(branches)
            .map_err(refused)?;
        for segment in segments {
            for (block, marks) in segment.marks.chunks(WORK_BLOCK).enumerate() {
                c.checkpoint(1)?;
                for (index, mark) in marks.iter().enumerate() {
                    if *mark == 0 {
                        continue;
                    }
                    let pc = segment.address + ((block * WORK_BLOCK + index) as u32) * GRANULE;
                    if mark & EXECUTED != 0 {
                        coverage.instructions.push(pc);
                    }
                    if mark & (TAKEN | FALLTHROUGH) != 0 {
                        coverage.branches.push(BranchCoverage {
                            site: pc,
                            taken: mark & TAKEN != 0,
                            fallthrough: mark & FALLTHROUGH != 0,
                        });
                    }
                }
            }
        }
        Ok((coverage, reservation))
    }
}
