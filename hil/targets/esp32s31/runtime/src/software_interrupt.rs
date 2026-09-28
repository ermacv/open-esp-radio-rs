//! The chip's four FROM_CPU software interrupt lines, each with one owner.
//!
//! Every line is taken: the wakes of core 0's and core 1's executors, and the
//! cross-hart samples of the hang watchdog and of the program-counter
//! profiler. Owners reach their line only through this module. A line's
//! owner is a [`Line`] variant whose discriminant is the line number, so two
//! owners of one line fail the build (a discriminant is assigned once), and
//! a fifth owner has no line to name: sharing one has to be decided here.

use esp_hal::interrupt::software::SoftwareInterrupt;

/// The owner of each line; the discriminant is its FROM_CPU line number.
#[repr(u8)]
pub(crate) enum Line {
    /// Core 0's executor wake.
    Executor0 = 0,
    /// Core 1's executor wake.
    #[cfg_attr(not(feature = "open-radio-hil"), allow(dead_code))]
    Executor1 = 1,
    /// The hang watchdog's sample of core 1.
    #[cfg_attr(
        not(all(feature = "open-radio-hil", not(feature = "memory-benchmark"))),
        allow(dead_code)
    )]
    HangWatchdog = 2,
    /// The program-counter profiler's sample of core 1.
    #[cfg_attr(not(feature = "pc-profile"), allow(dead_code))]
    Profiler = 3,
}

/// Core 0's executor wake, from the line's peripheral.
pub(crate) fn executor0(
    line: esp_hal::peripherals::FROM_CPU_INTR0<'static>,
) -> SoftwareInterrupt<'static, { Line::Executor0 as u8 }> {
    SoftwareInterrupt::new(line)
}

/// Core 1's executor wake, from the line's peripheral.
#[cfg(feature = "open-radio-hil")]
pub(crate) fn executor1(
    line: esp_hal::peripherals::FROM_CPU_INTR1<'static>,
) -> SoftwareInterrupt<'static, { Line::Executor1 as u8 }> {
    SoftwareInterrupt::new(line)
}

/// Core 1's executor wake again after the stack switch, which consumed and
/// forgot the peripheral before entering the new stack.
#[cfg(feature = "open-radio-hil")]
pub(crate) fn executor1_after_stack_switch() -> SoftwareInterrupt<'static, { Line::Executor1 as u8 }>
{
    // SAFETY: the line's one owner, core 1's executor, takes it back here
    // after its peripheral was consumed by the stack switch.
    #[allow(unsafe_code, reason = "the stack switch forgot the line's peripheral")]
    SoftwareInterrupt::new(unsafe { esp_hal::peripherals::FROM_CPU_INTR1::steal() })
}

/// The hang watchdog's line: its handler binding, raise and reset.
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
pub(crate) fn hang_watchdog() -> SoftwareInterrupt<'static, { Line::HangWatchdog as u8 }> {
    // SAFETY: the line has one owner, the hang watchdog; binding, raising
    // and resetting touch only this line.
    #[allow(unsafe_code, reason = "the line is shared by two cores")]
    SoftwareInterrupt::new(unsafe { esp_hal::peripherals::FROM_CPU_INTR2::steal() })
}

/// The program-counter profiler's line: its handler binding, raise and
/// reset.
#[cfg(feature = "pc-profile")]
pub(crate) fn profiler() -> SoftwareInterrupt<'static, { Line::Profiler as u8 }> {
    // SAFETY: the line has one owner, the profiler; binding, raising and
    // resetting touch only this line.
    #[allow(unsafe_code, reason = "the line is shared by two cores")]
    SoftwareInterrupt::new(unsafe { esp_hal::peripherals::FROM_CPU_INTR3::steal() })
}
