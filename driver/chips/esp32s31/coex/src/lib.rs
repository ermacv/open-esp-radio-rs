#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

use open_esp_radio_esp32s31_pac::RadioRegisters;

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
        let index = REVIEWED_TIMER_MAP[(self.0 - 1) as usize];
        if index == 0xff {
            None
        } else {
            Some(CoexTimerIndex(index))
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
pub struct CoexTimerIndex(u8);

impl CoexTimerIndex {
    pub const fn new(value: u8) -> Result<Self, CoexError> {
        if (value as usize) < COEX_TIMER_COUNT {
            Ok(Self(value))
        } else {
            Err(CoexError::InvalidTimer)
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexClient {
    Bluetooth = 0,
    Wifi = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexRequest {
    pub client: CoexClient,
    pub event: CoexEventId,
    /// Delay before the request must be granted. The vendor timer stores this
    /// converted value in the secondary target word.
    pub latency: u32,
    /// Requested ownership duration. The vendor timer stores this converted
    /// value in the primary configuration word.
    pub duration: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexTimerClock {
    pub selector: CoexClockSelector,
    /// Raw twelve-bit divider field sampled from `COEX_LP_CLK_CONF`.
    pub divider_field: u16,
    pub xtal_mhz: u32,
    pub real_chip: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexClockSelector {
    Selector1 = 1,
    Selector2 = 2,
    Selector4 = 4,
    Selector8 = 8,
}

impl CoexClockSelector {
    pub const fn from_bits(value: u8) -> Result<Self, CoexError> {
        match value {
            1 => Ok(Self::Selector1),
            2 => Ok(Self::Selector2),
            4 => Ok(Self::Selector4),
            8 => Ok(Self::Selector8),
            _ => Err(CoexError::UnsupportedClock),
        }
    }

    /// Validate the public divisor used by `coex_hw_timer_freq_set`.
    /// The register stores this value minus one.
    pub const fn accepts_divisor(self, divisor: u16) -> bool {
        divisor <= 4096
            && match self {
                Self::Selector8 => divisor == 1,
                Self::Selector4 => divisor >= 50,
                Self::Selector2 => divisor >= 40,
                Self::Selector1 => divisor >= 3,
            }
    }
}

impl CoexTimerClock {
    /// Decode the two fresh `COEX_LP_CLK_CONF` images sampled by the vendor
    /// helper. Keeping this interpretation in the executor-neutral core lets
    /// platform adapters retain MMIO ownership without duplicating bit logic.
    pub fn from_register_images(
        selector_image: u32,
        divider_image: u32,
        xtal_mhz: u32,
        real_chip: bool,
    ) -> Result<Self, CoexError> {
        Ok(Self {
            selector: CoexClockSelector::from_bits((selector_image & 0x0f) as u8)?,
            divider_field: ((divider_image >> 4) & 0x0fff) as u16,
            xtal_mhz,
            real_chip,
        })
    }

    /// Reproduce `coex_hw_timer_tick_get`, including its two-stage integer
    /// division for clock sources two and four.
    pub fn tick_image(self, value: u32) -> Result<u32, CoexError> {
        let numerator = u64::from(value) << 19;
        // The vendor extracts only COEX_LP_CLK_CONF bits 15:4.
        let divider = u64::from(self.divider_field & 0x0fff) + 1;
        let scale = match self.selector {
            CoexClockSelector::Selector8 => {
                if self.real_chip {
                    16_000_000_u64
                } else {
                    16_384_000_u64
                }
            }
            CoexClockSelector::Selector4 => {
                if self.xtal_mhz == 0 {
                    return Err(CoexError::UnsupportedClock);
                }
                (524_288_u64 * 1_000_000 * divider) / (u64::from(self.xtal_mhz) * 1_000_000)
            }
            CoexClockSelector::Selector2 => (524_288_u64 * 1_000_000 * divider) / 20_000_000,
            CoexClockSelector::Selector1 => 1,
        };
        if scale == 0 {
            return Err(CoexError::UnsupportedClock);
        }
        Ok((numerator / scale) as u32)
    }
}

/// Platform-owned access to the shared low-power clock configuration.
///
/// Sampling is deliberately mutable: each call represents the fresh MMIO
/// reads performed by one vendor `coex_hw_timer_tick_get` invocation.
pub trait CoexClockHardware {
    fn configure(&mut self, selector: CoexClockSelector, divisor: u16) -> Result<(), CoexError>;

    fn sample(&mut self) -> Result<CoexTimerClock, CoexError>;
}

pub trait CoexTimerHardware {
    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError>;
    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError>;
    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError>;
    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
}

/// Program one hardware timer exactly like `coex_hw_timer_set`.
///
/// Enabling the timer is deliberately separate because the vendor core first
/// completes all four fresh-read RMW operations and only then publishes the
/// timer through `coex_hw_timer_enable`.
pub fn program_timer<H: CoexTimerHardware, C: CoexClockHardware>(
    hardware: &mut H,
    clock: &mut C,
    index: CoexTimerIndex,
    client: CoexClient,
    pti: CoexPti,
    latency: u32,
    duration: u32,
) -> Result<(), CoexError> {
    hardware.configure_request(index, client, pti)?;
    let primary = clock.sample()?.tick_image(duration)?;
    hardware.set_primary_target(index, primary)?;
    let secondary = clock.sample()?.tick_image(latency)?;
    hardware.set_secondary_target(index, secondary)
}

impl CoexTimerHardware for RadioRegisters {
    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        self.configure_coex_timer(index.value(), client as u8, pti.value())
            .map_err(|_| CoexError::Hardware)
    }

    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.set_coex_timer_primary_target(index.value(), tick_image)
            .map_err(|_| CoexError::Hardware)
    }

    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.set_coex_timer_secondary_target(index.value(), tick_image)
            .map_err(|_| CoexError::Hardware)
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.enable_coex_timer(index.value())
            .map_err(|_| CoexError::Hardware)
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.disable_coex_timer(index.value())
            .map_err(|_| CoexError::Hardware)
    }

    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.force_coex_timer(index.value())
            .map_err(|_| CoexError::Hardware)
    }

    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.unforce_coex_timer(index.value())
            .map_err(|_| CoexError::Hardware)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexStatus {
    pub enabled: bool,
    pub active_timers: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexRequestOutcome {
    /// The event has no entry in the reviewed hardware timer map. This is a
    /// successful no-op in the vendor core, not an invalid API argument.
    Unmapped,
    Armed(CoexTimerIndex),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexReleaseOutcome {
    Unmapped,
    Released(CoexTimerIndex),
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
        for index in 0..COEX_TIMER_COUNT {
            if self.active[index].is_some() {
                hardware.disable(CoexTimerIndex(index as u8))?;
                self.active[index] = None;
            }
        }
        self.enabled = false;
        Ok(())
    }

    pub fn request<H: CoexTimerHardware, C: CoexClockHardware>(
        &mut self,
        hardware: &mut H,
        clock: &mut C,
        request: CoexRequest,
    ) -> Result<CoexRequestOutcome, CoexError> {
        if !self.enabled {
            return Err(CoexError::Disabled);
        }
        let Some(index) = request.event.timer_index() else {
            return Ok(CoexRequestOutcome::Unmapped);
        };
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
        self.active[index.0 as usize] = Some(request);
        Ok(CoexRequestOutcome::Armed(index))
    }

    pub fn release<H: CoexTimerHardware>(
        &mut self,
        hardware: &mut H,
        event: CoexEventId,
    ) -> Result<CoexReleaseOutcome, CoexError> {
        let Some(index) = event.timer_index() else {
            return Ok(CoexReleaseOutcome::Unmapped);
        };
        hardware.disable(index)?;
        self.active[index.0 as usize] = None;
        Ok(CoexReleaseOutcome::Released(index))
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

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    type OperationTrace = Rc<RefCell<std::vec::Vec<&'static str>>>;

    #[derive(Default)]
    struct TimerModel {
        programmed: Option<(u8, u8, u8, u32, u32)>,
        enabled: u8,
        disabled: u8,
        operations: OperationTrace,
    }

    impl CoexTimerHardware for TimerModel {
        fn configure_request(
            &mut self,
            index: CoexTimerIndex,
            client: CoexClient,
            pti: CoexPti,
        ) -> Result<(), CoexError> {
            self.programmed = Some((index.value(), client as u8, pti.value(), 0, 0));
            self.operations.borrow_mut().push("configure");
            Ok(())
        }
        fn set_primary_target(
            &mut self,
            _index: CoexTimerIndex,
            primary: u32,
        ) -> Result<(), CoexError> {
            self.programmed.as_mut().unwrap().3 = primary;
            self.operations.borrow_mut().push("primary");
            Ok(())
        }
        fn set_secondary_target(
            &mut self,
            _index: CoexTimerIndex,
            secondary: u32,
        ) -> Result<(), CoexError> {
            self.programmed.as_mut().unwrap().4 = secondary;
            self.operations.borrow_mut().push("secondary");
            Ok(())
        }
        fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
            self.enabled |= 1 << index.value();
            self.operations.borrow_mut().push("enable");
            Ok(())
        }
        fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
            self.disabled |= 1 << index.value();
            self.enabled &= !(1 << index.value());
            Ok(())
        }
        fn force(&mut self, _: CoexTimerIndex) -> Result<(), CoexError> {
            Ok(())
        }
        fn unforce(&mut self, _: CoexTimerIndex) -> Result<(), CoexError> {
            Ok(())
        }
    }

    struct ClockModel {
        clock: CoexTimerClock,
        samples: u8,
        operations: OperationTrace,
    }

    impl CoexClockHardware for ClockModel {
        fn configure(
            &mut self,
            selector: CoexClockSelector,
            divisor: u16,
        ) -> Result<(), CoexError> {
            if !selector.accepts_divisor(divisor) {
                return Err(CoexError::UnsupportedClock);
            }
            self.clock.selector = selector;
            self.clock.divider_field = divisor - 1;
            Ok(())
        }

        fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
            self.samples += 1;
            self.operations.borrow_mut().push("clock");
            Ok(self.clock)
        }
    }

    #[test]
    fn reviewed_pti_table_is_complete_and_four_bit_clean() {
        let mut table = CoexPtiTable::reviewed_vendor();
        assert_eq!(table.as_bytes().len(), 48);
        assert!(table.as_bytes().iter().all(|value| *value <= 0x0f));
        assert_eq!(table.pti(CoexEventId::new(1).unwrap()).value(), 5);
        assert_eq!(table.pti(CoexEventId::new(3).unwrap()).value(), 7);
        assert_eq!(table.pti(CoexEventId::new(10).unwrap()).value(), 3);
        assert_eq!(table.pti(CoexEventId::new(15).unwrap()).value(), 1);
        table.set(CoexEventId::new(47).unwrap(), CoexPti::new(3).unwrap());
        assert_eq!(table.pti(CoexEventId::new(47).unwrap()).value(), 3);
    }

    #[test]
    fn reviewed_event_durations_match_the_vendor_parameter_object() {
        let durations = CoexEventDurations::reviewed_vendor();
        assert_eq!(
            durations.as_words(),
            &[25_000, 20_000, 5_000, 25_000, 50_000]
        );
        for (event, expected) in [
            (4, 25_000),
            (7, 20_000),
            (9, 5_000),
            (45, 25_000),
            (46, 50_000),
        ] {
            assert_eq!(
                durations.duration(CoexEventId::new(event).unwrap()),
                Some(expected)
            );
        }
        assert_eq!(durations.duration(CoexEventId::new(8).unwrap()), None);
    }

    #[test]
    fn timer_map_rejects_ff_entries_and_reaches_every_timer() {
        let mapped: [u8; 6] = [1, 2, 36, 38, 45, 46];
        let indices = mapped.map(|event| {
            CoexEventId::new(event)
                .unwrap()
                .timer_index()
                .unwrap()
                .value()
        });
        assert_eq!(indices, [0, 1, 4, 2, 3, 3]);
        assert_eq!(CoexEventId::new(3).unwrap().timer_index(), None);
    }

    #[test]
    fn clock_conversion_matches_instruction_level_constants() {
        let fast = CoexTimerClock {
            selector: CoexClockSelector::Selector8,
            divider_field: 0,
            xtal_mhz: 40,
            real_chip: true,
        };
        assert_eq!(fast.tick_image(1_000).unwrap(), 32);
        assert_eq!(fast.tick_image(1_000_000).unwrap(), 32_768);
        let emulator = CoexTimerClock {
            real_chip: false,
            ..fast
        };
        assert_eq!(emulator.tick_image(1_000_000).unwrap(), 32_000);
        let slow = CoexTimerClock {
            selector: CoexClockSelector::Selector1,
            divider_field: 0,
            xtal_mhz: 40,
            real_chip: true,
        };
        assert_eq!(slow.tick_image(1).unwrap(), 524_288);

        let source_two = CoexTimerClock {
            selector: CoexClockSelector::Selector2,
            divider_field: 39,
            xtal_mhz: 40,
            real_chip: true,
        };
        assert_eq!(source_two.tick_image(1_000).unwrap(), 500);

        let source_four = CoexTimerClock {
            selector: CoexClockSelector::Selector4,
            divider_field: 49,
            xtal_mhz: 40,
            real_chip: true,
        };
        assert_eq!(source_four.tick_image(1_000).unwrap(), 800);
    }

    #[test]
    fn request_programs_then_enables_and_release_disables() {
        let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
        let operations = OperationTrace::default();
        let mut hardware = TimerModel {
            operations: operations.clone(),
            ..TimerModel::default()
        };
        core.enable();
        let request = CoexRequest {
            client: CoexClient::Wifi,
            event: CoexEventId::new(1).unwrap(),
            latency: 2,
            duration: 3,
        };
        let mut clock = ClockModel {
            clock: CoexTimerClock {
                selector: CoexClockSelector::Selector1,
                divider_field: 0,
                xtal_mhz: 40,
                real_chip: true,
            },
            samples: 0,
            operations: operations.clone(),
        };
        assert_eq!(
            core.request(&mut hardware, &mut clock, request).unwrap(),
            CoexRequestOutcome::Armed(CoexTimerIndex::new(0).unwrap())
        );
        assert_eq!(hardware.programmed, Some((0, 1, 5, 1_572_864, 1_048_576)));
        assert_eq!(clock.samples, 2);
        assert_eq!(
            operations.borrow().as_slice(),
            [
                "configure",
                "clock",
                "primary",
                "clock",
                "secondary",
                "enable"
            ]
        );
        assert_eq!(hardware.enabled, 1);
        assert_eq!(
            core.release(&mut hardware, request.event).unwrap(),
            CoexReleaseOutcome::Released(CoexTimerIndex::new(0).unwrap())
        );
        assert_eq!(hardware.disabled, 1);
        assert_eq!(core.status().active_timers, 0);
    }

    #[test]
    fn unmapped_events_are_successful_no_ops() {
        let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
        let mut hardware = TimerModel::default();
        let mut clock = ClockModel {
            clock: CoexTimerClock {
                selector: CoexClockSelector::Selector8,
                divider_field: 0,
                xtal_mhz: 40,
                real_chip: true,
            },
            samples: 0,
            operations: OperationTrace::default(),
        };
        core.enable();
        let event = CoexEventId::new(0).unwrap();
        assert_eq!(
            core.request(
                &mut hardware,
                &mut clock,
                CoexRequest {
                    client: CoexClient::Wifi,
                    event,
                    latency: 0,
                    duration: 100,
                },
            ),
            Ok(CoexRequestOutcome::Unmapped)
        );
        assert_eq!(
            core.release(&mut hardware, event),
            Ok(CoexReleaseOutcome::Unmapped)
        );
        assert!(hardware.operations.borrow().is_empty());
        assert_eq!(clock.samples, 0);
    }

    #[test]
    fn clock_configuration_matches_vendor_domains() {
        assert!(CoexClockSelector::Selector8.accepts_divisor(1));
        assert!(!CoexClockSelector::Selector8.accepts_divisor(2));
        assert!(CoexClockSelector::Selector4.accepts_divisor(50));
        assert!(!CoexClockSelector::Selector4.accepts_divisor(49));
        assert!(CoexClockSelector::Selector2.accepts_divisor(40));
        assert!(!CoexClockSelector::Selector2.accepts_divisor(39));
        assert!(CoexClockSelector::Selector1.accepts_divisor(3));
        assert!(!CoexClockSelector::Selector1.accepts_divisor(2));
        assert!(CoexClockSelector::Selector1.accepts_divisor(4096));
        assert!(!CoexClockSelector::Selector1.accepts_divisor(4097));
    }
}
