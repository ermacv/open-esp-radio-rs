#![no_std]

use core::future::Future;

pub use open_esp_radio_pac_esp32s31::RadioRegisters;

/// Unique application-visible owner of the radio peripheral.
pub struct Radio {
    registers: RadioRegisters,
}

impl Radio {
    /// Construct the radio owner after proving that ROM/vendor code no longer
    /// owns or mutates the radio.
    ///
    /// # Safety
    ///
    /// The caller must uphold unique hardware ownership for the value's
    /// lifetime.
    pub const unsafe fn steal() -> Self {
        Self {
            registers: unsafe { RadioRegisters::steal() },
        }
    }

    pub const fn registers(&self) -> &RadioRegisters {
        &self.registers
    }

    pub fn registers_mut(&mut self) -> &mut RadioRegisters {
        &mut self.registers
    }
}

/// Executor-neutral source of asynchronous deadlines.
pub trait AsyncDelay {
    type Error;

    fn delay_micros(&mut self, micros: u32) -> impl Future<Output = Result<(), Self::Error>> + '_;
}

/// Executor-neutral interrupt/event edge.
pub trait AsyncEvent {
    type Event;
    type Error;

    fn wait(&mut self) -> impl Future<Output = Result<Self::Event, Self::Error>> + '_;
}
