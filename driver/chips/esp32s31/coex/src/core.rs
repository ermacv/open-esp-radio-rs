use crate::{
    COEX_TIMER_COUNT, CoexClockHardware, CoexError, CoexEventDurations, CoexEventId, CoexPti,
    CoexPtiTable, CoexRequest, CoexTimerHardware, CoexTimerIndex, program_timer,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexStatus {
    pub enabled: bool,
    pub active_timers: u8,
}

pub struct CoexCore {
    enabled: bool,
    active: [Option<CoexRequest>; COEX_TIMER_COUNT],
    pti: CoexPtiTable,
    durations: CoexEventDurations,
}

impl CoexCore {
    pub const fn new(pti: CoexPtiTable) -> Self {
        Self {
            enabled: false,
            active: [None; COEX_TIMER_COUNT],
            pti,
            durations: CoexEventDurations::reviewed_vendor(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable<H: CoexTimerHardware>(&mut self, hardware: &mut H) -> Result<(), CoexError> {
        for index in CoexTimerIndex::ALL {
            if self.active[usize::from(index.value())].is_some() {
                hardware.disable(index)?;
                self.active[usize::from(index.value())] = None;
            }
        }
        self.enabled = false;
        Ok(())
    }

    /// Arm the mapped vendor timer for one event.
    ///
    /// The explicit enabled guard is Rust lifecycle policy. With that
    /// precondition satisfied, unmapped events follow `coex_core_request` and
    /// return the vendor invalid-event status instead of becoming no-ops.
    pub fn request<H: CoexTimerHardware, C: CoexClockHardware>(
        &mut self,
        hardware: &mut H,
        clock: &mut C,
        request: CoexRequest,
    ) -> Result<CoexTimerIndex, CoexError> {
        if !self.enabled {
            return Err(CoexError::Disabled);
        }
        let index = request.event.timer_index().ok_or(CoexError::InvalidEvent)?;
        program_timer(
            hardware,
            clock,
            index,
            request.client,
            self.pti.pti(request.event),
            request.latency,
            request.duration,
        )?;
        hardware.enable(index)?;
        self.active[usize::from(index.value())] = Some(request);
        Ok(index)
    }

    pub fn release<H: CoexTimerHardware>(
        &mut self,
        hardware: &mut H,
        event: CoexEventId,
    ) -> Result<CoexTimerIndex, CoexError> {
        let index = event.timer_index().ok_or(CoexError::InvalidEvent)?;
        hardware.disable(index)?;
        self.active[usize::from(index.value())] = None;
        Ok(index)
    }

    pub fn status(&self) -> CoexStatus {
        let mut active_timers = 0_u8;
        for (index, request) in self.active.iter().enumerate() {
            if request.is_some() {
                active_timers |= 1 << index;
            }
        }
        CoexStatus {
            enabled: self.enabled,
            active_timers,
        }
    }

    pub const fn pti(&self) -> &CoexPtiTable {
        &self.pti
    }

    pub fn set_pti(&mut self, event: CoexEventId, pti: CoexPti) {
        self.pti.set(event, pti);
    }

    pub const fn event_duration(&self, event: CoexEventId) -> Option<u32> {
        self.durations.duration(event)
    }
}
