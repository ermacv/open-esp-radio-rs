//! Direct Test Mode session over the portable radio event contract.
//!
//! [`DtmSession`] plans one test event at a time. A transmitter test places
//! its packets on the `I(L)` grid of Core 6.0 Vol 6 Part F section 4.1.6 and
//! skips grid points that are no longer admissible; a receiver test listens
//! in back-to-back windows and counts every packet with a valid CRC. Ending a
//! test first lets the event in progress leave the schedule.

use oer_bluetooth_radio::{
    EventId, EventResult, RadioDuration, RadioInstant, RadioOutcome, RadioRequest, RadioTiming,
    RadioWindow, TestChannel, TestPayloadType, TestPhy, TestReceive, TestReport, TestTransmit,
    TxPower,
};

mod payload;

pub use payload::DtmPayloadPattern;

/// Longest LE Test packet payload.
pub const DTM_MAX_PAYLOAD: usize = 255;

/// Length of one receiver listening window.
pub const DTM_RECEIVE_WINDOW: RadioDuration = RadioDuration::from_micros(5_000);

/// Slack between planning an event and the earliest instant the backend
/// admits, covering the time until the request reaches it.
pub const DTM_PLANNING_SLACK: RadioDuration = RadioDuration::from_micros(500);

/// One test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTest {
    /// Transmit test packets.
    Transmit {
        /// RF channel.
        channel: TestChannel,
        /// PHY.
        phy: TestPhy,
        /// Payload pattern.
        pattern: DtmPayloadPattern,
        /// Payload length in bytes.
        length: u8,
    },
    /// Receive and count test packets.
    Receive {
        /// RF channel.
        channel: TestChannel,
        /// PHY.
        phy: TestPhy,
    },
}

/// Why a test did not start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmStartError {
    /// A test is running or ending.
    Active,
    /// Coded transmitter timing is not implemented.
    UnsupportedPhy,
}

/// Result of ending a test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmStop {
    /// The test stopped; `received` counts the receiver's packets and is zero
    /// for a transmitter.
    Stopped {
        /// Received packets.
        received: u16,
    },
    /// Cancel this event and wait for it to end.
    Draining(EventId),
}

/// Counters of the current test.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DtmCounters {
    /// Events that ran.
    pub executed: u32,
    /// Events that left the schedule without running.
    pub not_executed: u32,
    /// Receiver events that returned no packet.
    pub empty: u32,
    /// Receiver events whose packet failed its check.
    pub failed: u32,
    /// Receiver packets with a valid CRC.
    pub received: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Running(DtmTest),
    Ending(DtmTest),
}

/// Plans the events of one Direct Test Mode test.
#[derive(Debug)]
pub struct DtmSession {
    phase: Phase,
    tx_power: TxPower,
    next_id: u32,
    outstanding: Option<EventId>,
    last: Option<RadioWindow>,
    recurring: bool,
    counters: DtmCounters,
}

impl DtmSession {
    /// A session whose events use `tx_power`. Event identities start at
    /// `first_id` and must not collide with the caller's other events.
    pub const fn new(tx_power: TxPower, first_id: u32) -> Self {
        Self {
            phase: Phase::Idle,
            tx_power,
            next_id: first_id,
            outstanding: None,
            last: None,
            recurring: false,
            counters: DtmCounters {
                executed: 0,
                not_executed: 0,
                empty: 0,
                failed: 0,
                received: 0,
            },
        }
    }

    /// Whether a test is running or ending.
    pub const fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Counters of the current or last test.
    pub const fn counters(&self) -> DtmCounters {
        self.counters
    }

    /// Start `test`.
    pub fn start(&mut self, test: DtmTest) -> Result<(), DtmStartError> {
        if self.is_active() {
            return Err(DtmStartError::Active);
        }
        if let DtmTest::Transmit { phy, length, .. } = test
            && transmit_air_time(phy, length).is_none()
        {
            return Err(DtmStartError::UnsupportedPhy);
        }
        self.phase = Phase::Running(test);
        self.last = None;
        self.recurring = false;
        self.counters = DtmCounters::default();
        Ok(())
    }

    /// Stop planning. An event in progress must end first.
    pub fn end(&mut self) -> DtmStop {
        let test = match self.phase {
            Phase::Idle => return DtmStop::Stopped { received: 0 },
            Phase::Running(test) | Phase::Ending(test) => test,
        };
        match self.outstanding {
            Some(id) => {
                self.phase = Phase::Ending(test);
                DtmStop::Draining(id)
            }
            None => {
                self.phase = Phase::Idle;
                DtmStop::Stopped {
                    received: received(test, self.counters),
                }
            }
        }
    }

    /// The next event request, when the session needs one.
    ///
    /// `now` is a fresh backend time and `timing` its admission rule. A
    /// transmitter request borrows its payload from `payload`.
    pub fn next_request<'payload>(
        &mut self,
        now: RadioInstant,
        timing: RadioTiming,
        payload: &'payload mut [u8; DTM_MAX_PAYLOAD],
    ) -> Option<RadioRequest<'payload>> {
        let Phase::Running(test) = self.phase else {
            return None;
        };
        if self.outstanding.is_some() {
            return None;
        }
        let earliest = earliest_anchor(now, timing)?;
        let id = EventId::new(self.next_id);
        let request = match test {
            DtmTest::Transmit {
                channel,
                phy,
                pattern,
                length,
            } => {
                let air = transmit_air_time(phy, length)?;
                let interval = u64::from(packet_interval(air).as_micros());
                let anchor = match self.last {
                    None => earliest,
                    Some(last) => {
                        let first = last.start().as_micros() + interval;
                        let late = earliest.as_micros().saturating_sub(first);
                        RadioInstant::from_micros(first + late.div_ceil(interval) * interval)
                    }
                };
                let window = RadioWindow::new(anchor, air).ok()?;
                let bytes = &mut payload[..usize::from(length)];
                pattern.fill(bytes);
                self.last = Some(window);
                RadioRequest::TestTransmit(TestTransmit {
                    id,
                    channel,
                    phy,
                    window,
                    tx_power: self.tx_power,
                    payload_type: TestPayloadType::new(pattern.payload_type())
                        .expect("every pattern has a test payload type"),
                    payload: bytes,
                })
            }
            DtmTest::Receive { channel, phy } => {
                let anchor = match self.last {
                    Some(last) if last.end() > earliest => last.end(),
                    _ => earliest,
                };
                let window = RadioWindow::new(anchor, DTM_RECEIVE_WINDOW).ok()?;
                self.last = Some(window);
                let recurring = core::mem::replace(&mut self.recurring, true);
                RadioRequest::TestReceive(TestReceive {
                    id,
                    channel,
                    phy,
                    window,
                    recurring,
                    tx_power: self.tx_power,
                })
            }
        };
        self.outstanding = Some(id);
        Some(request)
    }

    /// The backend refused the planned request; plan again later.
    pub fn refused(&mut self) {
        self.outstanding = None;
    }

    /// Account one outcome. Outcomes of other events are ignored.
    pub fn observe(&mut self, outcome: RadioOutcome<'_>) {
        match outcome {
            RadioOutcome::TestReport { id, report } if Some(id) == self.outstanding => match report
            {
                TestReport::Nothing => self.counters.empty += 1,
                TestReport::Received { .. } => self.counters.received += 1,
                TestReport::Failed => self.counters.failed += 1,
            },
            RadioOutcome::EventEnded { id, result } if Some(id) == self.outstanding => {
                match result {
                    EventResult::Executed { .. } => self.counters.executed += 1,
                    EventResult::NotExecuted => self.counters.not_executed += 1,
                }
                self.outstanding = None;
                self.next_id = self.next_id.wrapping_add(1);
                if let Phase::Ending(_) = self.phase {
                    self.phase = Phase::Idle;
                }
            }
            _ => {}
        }
    }

    /// The receiver count once an ending test has drained.
    pub fn drained(&self) -> Option<u16> {
        match (self.phase, self.outstanding) {
            (Phase::Idle, None) => Some(self.counters.received.min(u32::from(u16::MAX)) as u16),
            _ => None,
        }
    }
}

fn received(test: DtmTest, counters: DtmCounters) -> u16 {
    match test {
        DtmTest::Transmit { .. } => 0,
        DtmTest::Receive { .. } => counters.received.min(u32::from(u16::MAX)) as u16,
    }
}

fn earliest_anchor(now: RadioInstant, timing: RadioTiming) -> Option<RadioInstant> {
    now.checked_add(timing.preparation_lead)?
        .checked_add(timing.admission_guard)?
        .checked_add(DTM_PLANNING_SLACK)
}

/// Air time of one LE Test packet: preamble, access address, header,
/// payload and CRC.
fn transmit_air_time(phy: TestPhy, length: u8) -> Option<RadioDuration> {
    let length = u32::from(length);
    match phy {
        TestPhy::Le1M => Some(RadioDuration::from_micros((1 + 4 + 2 + length + 3) * 8)),
        TestPhy::Le2M => Some(RadioDuration::from_micros((2 + 4 + 2 + length + 3) * 4)),
        TestPhy::LeCodedS8 | TestPhy::LeCodedS2 => None,
    }
}

/// `I(L) = ceil((L + 249) / 625) * 625` microseconds for a packet of `L`
/// microseconds.
fn packet_interval(air: RadioDuration) -> RadioDuration {
    RadioDuration::from_micros((air.as_micros() + 249).div_ceil(625) * 625)
}

#[cfg(test)]
mod tests;
