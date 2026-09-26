//! Shared coexistence mechanism owned by the radio arbiter.
//!
//! The arbiter keeps the coexistence priority table ([`CoexPtiTable`]) and
//! lends the timer bank ([`CoexTimerBank`]) through its lease. Both are
//! mechanism: which event requests which timer, and when, is the coexistence
//! driver's policy, and every protocol programs its own MAC PTI registers from
//! the values read here. A timer write or a PTI value does not report RF
//! admission or grant revocation; the hardware arbitrates by priority and
//! gives the CPU no grant acknowledgement.

use oer_esp32s31_pac::{
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerRegister,
    CoexistenceLowPowerClockObservation, SharedRadioRegisters,
};

/// Number of coexistence events, event zero included.
pub const COEX_EVENT_COUNT: usize = 49;

/// Complete `coex_pti_tab` of esp-coex-lib
/// `02c57071231dbe9f7555afcbb8ef86e35f517292`
/// (`esp32s31/libcoexist.a` sha256
/// `7fdcdc9d8e225379af71953d9a67d03076e8358e0231b611be7d6f487824f0b7`,
/// `coexist_core.o` section `.dram1.2`, 49 bytes), the revision paired with
/// the pinned esp-phy-lib `b88e4b76`. Events 0 through 47 equal the earlier
/// reviewed archive `9b8f55b1`; event 48 is new. Index is the
/// event number. Events 1, 3, 10 and 15 are the cold Wi-Fi MAC priorities
/// (5, 7, 3 and 1); event 48 is the PHY grant-protect request (15).
const VENDOR_PTI_TABLE: [u8; COEX_EVENT_COUNT] = [
    0x0a, 0x05, 0x07, 0x07, 0x0a, 0x01, 0x01, 0x01, 0x01, 0x07, 0x03, 0x02, 0x01, 0x01, 0x01, 0x01,
    0x04, 0x09, 0x04, 0x04, 0x09, 0x04, 0x09, 0x04, 0x04, 0x05, 0x05, 0x05, 0x05, 0x04, 0x04, 0x04,
    0x04, 0x02, 0x02, 0x02, 0x0f, 0x0a, 0x04, 0x0e, 0x00, 0x0c, 0x08, 0x03, 0x01, 0x0a, 0x0a, 0x0f,
    0x0f,
];

/// One coexistence event number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexEventId(u8);

impl CoexEventId {
    /// An event of the vendor table, or `None` outside it.
    pub const fn new(value: u8) -> Option<Self> {
        if (value as usize) < COEX_EVENT_COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// One four-bit coexistence priority.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CoexPti(u8);

impl CoexPti {
    /// A priority of the four-bit hardware domain, or `None` above it.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 0x0f {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Priority of every coexistence event.
///
/// The vendor coexistence scheduler changes entries at run time; the table
/// is therefore shared state of the arbiter, not a constant of any protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPtiTable([u8; COEX_EVENT_COUNT]);

impl CoexPtiTable {
    /// The vendor cold table.
    pub const VENDOR: Self = Self(VENDOR_PTI_TABLE);

    pub const fn pti(&self, event: CoexEventId) -> CoexPti {
        // Every byte is four-bit clean: the vendor table is, and `set` takes
        // only a checked priority.
        CoexPti(self.0[event.0 as usize])
    }

    pub fn set(&mut self, event: CoexEventId, pti: CoexPti) {
        self.0[event.0 as usize] = pti.0;
    }

    pub const fn as_bytes(&self) -> &[u8; COEX_EVENT_COUNT] {
        &self.0
    }
}

/// Borrowed authority over the five coexistence hardware timers.
///
/// Every method is one finite register transaction. A write does not report
/// RF admission or grant revocation.
pub struct CoexTimerBank<'registers> {
    registers: &'registers mut SharedRadioRegisters,
}

impl<'registers> CoexTimerBank<'registers> {
    pub(crate) fn from_owned(registers: &'registers mut SharedRadioRegisters) -> Self {
        Self { registers }
    }

    /// Publish the client and PTI request fields of one timer.
    pub fn configure(
        &mut self,
        timer: CoexTimerRegister,
        client: CoexTimerClientValue,
        pti: CoexTimerPtiValue,
    ) {
        self.registers.configure_coex_timer(timer, client, pti);
    }

    /// Publish one already converted primary target tick image.
    pub fn set_primary_target(&mut self, timer: CoexTimerRegister, tick_image: u32) {
        self.registers
            .set_coex_timer_primary_target(timer, tick_image);
    }

    /// Publish one already converted secondary target tick image.
    pub fn set_secondary_target(&mut self, timer: CoexTimerRegister, tick_image: u32) {
        self.registers
            .set_coex_timer_secondary_target(timer, tick_image);
    }

    pub fn enable(&mut self, timer: CoexTimerRegister) {
        self.registers.enable_coex_timer(timer);
    }

    pub fn disable(&mut self, timer: CoexTimerRegister) {
        self.registers.disable_coex_timer(timer);
    }

    pub fn force(&mut self, timer: CoexTimerRegister) {
        self.registers.force_coex_timer(timer);
    }

    pub fn unforce(&mut self, timer: CoexTimerRegister) {
        self.registers.unforce_coex_timer(timer);
    }

    /// Sample the shared coexistence low-power clock selection once.
    ///
    /// Each call performs fresh reads; `None` reports an unreviewed encoding.
    pub fn sample_low_power_clock(&mut self) -> Option<CoexistenceLowPowerClockObservation> {
        self.registers.sample_coexistence_low_power_clock()
    }
}

#[cfg(test)]
mod tests;
