pub const COEX_EVENT_COUNT: usize = 48;
pub const COEX_TIMER_COUNT: usize = 5;

const REVIEWED_PRIORITY_TABLE: [u8; COEX_EVENT_COUNT] = [
    0x0a, 0x05, 0x07, 0x07, 0x0a, 0x01, 0x01, 0x01, 0x01, 0x07, 0x03, 0x02, 0x01, 0x01, 0x01, 0x01,
    0x04, 0x09, 0x04, 0x04, 0x09, 0x04, 0x09, 0x04, 0x04, 0x05, 0x05, 0x05, 0x05, 0x04, 0x04, 0x04,
    0x04, 0x02, 0x02, 0x02, 0x0f, 0x0a, 0x04, 0x0e, 0x00, 0x0c, 0x08, 0x03, 0x01, 0x0a, 0x0a, 0x0f,
];

// Complete `coex_core_timer_idx_get` switch image. Element zero corresponds
// to event one; 0xff means that the event has no hardware timer.
const REVIEWED_TIMER_MAP: [u8; 46] = [
    0x00, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0x04, 0xff, 0x02, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x03, 0x03,
];

// Complete `g_coex_param` initializer. `coex_core_event_duration_get` maps
// these entries to events 4, 7, 9, 45 and 46 respectively.
const REVIEWED_EVENT_DURATIONS: [u32; 5] = [25_000, 20_000, 5_000, 25_000, 50_000];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexError {
    InvalidEvent,
    InvalidPti,
    InvalidTimer,
    UnsupportedClock,
    Disabled,
    Hardware,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexEventId(u8);

impl CoexEventId {
    pub const fn new(value: u8) -> Result<Self, CoexError> {
        if (value as usize) < COEX_EVENT_COUNT {
            Ok(Self(value))
        } else {
            Err(CoexError::InvalidEvent)
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    pub const fn timer_index(self) -> Option<CoexTimerIndex> {
        if self.0 == 0 || self.0 > 46 {
            return None;
        }
        match REVIEWED_TIMER_MAP[(self.0 - 1) as usize] {
            0 => Some(CoexTimerIndex::Timer0),
            1 => Some(CoexTimerIndex::Timer1),
            2 => Some(CoexTimerIndex::Timer2),
            3 => Some(CoexTimerIndex::Timer3),
            4 => Some(CoexTimerIndex::Timer4),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPti(u8);

impl CoexPti {
    pub const fn new(value: u8) -> Result<Self, CoexError> {
        if value <= 0x0f {
            Ok(Self(value))
        } else {
            Err(CoexError::InvalidPti)
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPtiTable([u8; COEX_EVENT_COUNT]);

impl CoexPtiTable {
    pub const fn reviewed_vendor() -> Self {
        Self(REVIEWED_PRIORITY_TABLE)
    }

    pub const fn pti(self, event: CoexEventId) -> CoexPti {
        // Every byte of the reviewed table is four-bit clean.
        CoexPti(self.0[event.0 as usize])
    }

    pub fn set(&mut self, event: CoexEventId, pti: CoexPti) {
        self.0[event.0 as usize] = pti.0;
    }

    pub const fn as_bytes(&self) -> &[u8; COEX_EVENT_COUNT] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexEventDurations([u32; 5]);

impl CoexEventDurations {
    pub const fn reviewed_vendor() -> Self {
        Self(REVIEWED_EVENT_DURATIONS)
    }

    pub const fn duration(self, event: CoexEventId) -> Option<u32> {
        let index = match event.value() {
            4 => 0,
            7 => 1,
            9 => 2,
            45 => 3,
            46 => 4,
            _ => return None,
        };
        Some(self.0[index])
    }

    pub const fn as_words(&self) -> &[u32; 5] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexTimerIndex {
    Timer0 = 0,
    Timer1 = 1,
    Timer2 = 2,
    Timer3 = 3,
    Timer4 = 4,
}

impl CoexTimerIndex {
    pub const ALL: [Self; COEX_TIMER_COUNT] = [
        Self::Timer0,
        Self::Timer1,
        Self::Timer2,
        Self::Timer3,
        Self::Timer4,
    ];

    pub const fn new(value: u8) -> Result<Self, CoexError> {
        match value {
            0 => Ok(Self::Timer0),
            1 => Ok(Self::Timer1),
            2 => Ok(Self::Timer2),
            3 => Ok(Self::Timer3),
            4 => Ok(Self::Timer4),
            _ => Err(CoexError::InvalidTimer),
        }
    }

    pub const fn value(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexClient {
    Bluetooth = 0,
    Wifi = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexClientRequest {
    pub event: CoexEventId,
    /// Delay before the request must be granted. The vendor timer stores this
    /// converted value in the secondary target word.
    pub latency: u32,
    /// Requested ownership duration. The vendor timer stores this converted
    /// value in the primary configuration word.
    pub duration: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CoexRequest {
    pub(crate) client: CoexClient,
    pub(crate) request: CoexClientRequest,
}
