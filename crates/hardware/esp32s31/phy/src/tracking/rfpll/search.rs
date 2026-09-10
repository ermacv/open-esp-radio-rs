//! Bounded capacitor search. Actions describe completed helper operations;
//! the additional five-microsecond settle is separate from `phy_write_pll_cap`.
//! No action is a proof of RF access or of synthesizer lock.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Accepted,
    Increase,
    Decrease,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    ReadInitialCap,
    EnableSearch,
    /// Signed helper input. The helper's clamping does not change the value
    /// accumulated by the search when the status subsequently accepts it.
    WriteCap(i16),
    DelayMicros(u32),
    ReadStatus,
    Complete(Outcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    InitialCap(u16),
    SearchEnabled,
    CapWritten(i16),
    DelayElapsed(u32),
    Status(Status),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub initial_cap: u16,
    /// Requested final helper input, not readback of the programmed capacitor.
    pub selected_cap: u16,
    pub accepted_samples: u8,
}

impl Outcome {
    pub const fn delta(self) -> i16 {
        self.selected_cap.wrapping_sub(self.initial_cap) as i16
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    ReadInitial,
    Enable,
    WriteCandidate,
    Settle,
    ReadStatus,
    WriteFinal,
    Complete,
}

/// A single non-cloneable search. Each direction permits at most ten samples
/// and stops after two direction-specific boundary observations. These need
/// not be consecutive; other statuses do not reset the boundary count.
#[derive(Debug, Eq, PartialEq)]
pub struct Search {
    step: Step,
    phase: Phase,
    initial: u16,
    offset: u8,
    boundaries: u8,
    sum: u16,
    accepted: u8,
}

impl Default for Search {
    fn default() -> Self {
        Self::new()
    }
}

impl Search {
    pub const fn new() -> Self {
        Self {
            step: Step::ReadInitial,
            phase: Phase::Down,
            initial: 0,
            offset: 0,
            boundaries: 0,
            sum: 0,
            accepted: 0,
        }
    }

    const fn candidate(&self) -> u16 {
        match self.phase {
            Phase::Down => self.initial.wrapping_sub(self.offset as u16),
            Phase::Up => self
                .initial
                .wrapping_add(1)
                .wrapping_add(self.offset as u16),
        }
    }

    const fn outcome(&self) -> Outcome {
        Outcome {
            initial_cap: self.initial,
            accepted_samples: self.accepted,
            selected_cap: if self.accepted == 0 {
                self.initial
            } else {
                self.sum / self.accepted as u16
            },
        }
    }

    pub const fn action(&self) -> Action {
        match self.step {
            Step::ReadInitial => Action::ReadInitialCap,
            Step::Enable => Action::EnableSearch,
            Step::WriteCandidate => Action::WriteCap(self.candidate() as i16),
            Step::Settle => Action::DelayMicros(5),
            Step::ReadStatus => Action::ReadStatus,
            Step::WriteFinal => Action::WriteCap(self.outcome().selected_cap as i16),
            Step::Complete => Action::Complete(self.outcome()),
        }
    }

    pub fn advance(&mut self, completion: Completion) -> Result<(), Error> {
        self.step = match (self.step, completion) {
            (Step::ReadInitial, Completion::InitialCap(cap)) => {
                self.initial = cap;
                Step::Enable
            }
            (Step::Enable, Completion::SearchEnabled) => Step::WriteCandidate,
            (Step::WriteCandidate, Completion::CapWritten(cap))
                if cap == self.candidate() as i16 =>
            {
                Step::Settle
            }
            (Step::Settle, Completion::DelayElapsed(5)) => Step::ReadStatus,
            (Step::ReadStatus, Completion::Status(status)) => {
                if status == Status::Accepted {
                    self.sum = self.sum.wrapping_add(self.candidate());
                    self.accepted += 1;
                } else if matches!(
                    (self.phase, status),
                    (Phase::Down, Status::Increase) | (Phase::Up, Status::Decrease)
                ) {
                    self.boundaries += 1;
                }
                self.offset += 1;
                if self.offset == 10 || self.boundaries == 2 {
                    match self.phase {
                        Phase::Down => {
                            self.phase = Phase::Up;
                            self.offset = 0;
                            self.boundaries = 0;
                            Step::WriteCandidate
                        }
                        Phase::Up => Step::WriteFinal,
                    }
                } else {
                    Step::WriteCandidate
                }
            }
            (Step::WriteFinal, Completion::CapWritten(cap))
                if cap == self.outcome().selected_cap as i16 =>
            {
                Step::Complete
            }
            (Step::Complete, _) => return Err(Error::AlreadyComplete),
            _ => return Err(Error::WrongCompletion),
        };
        Ok(())
    }
}
