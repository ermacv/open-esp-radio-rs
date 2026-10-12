//! The chip's four FROM_CPU software interrupt lines, each with one owner.
//!
//! The lines are, in order: the wakes of core 0's and core 1's executors,
//! core 1's sample, and the IEEE 802.15.4 doorbell (#668). Owners reach their
//! line only through this module. A line's owner is a [`Line`] variant whose
//! discriminant is the line number, so two owners of one line fail the build
//! (a discriminant is assigned once), and a fifth owner has no line to name:
//! sharing one has to be decided here.
//!
//! Core 1's sample is one line shared by its requesters, the hang watchdog
//! and the program-counter profiler. Core 0 sets the requester's flag and
//! raises the line ([`request_core1_sample`]); [`sample_core1`], bound on
//! core 1 at `Priority8`, serves every pending requester from one entry
//! (`oer_hil_agent::sample_request`).
//!
//! Line 3 is the IEEE 802.15.4 doorbell's, at the MAC interrupt's level and
//! core (`Priority1`, core 0). Until #668 binds the doorbell, the
//! interrupt-latency probe (`interrupt-latency-probe`, #643) takes it, so the
//! probe times the doorbell's own line.

use esp_hal::interrupt::software::SoftwareInterrupt;
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
use oer_hil_agent::sample_request::{Requester, SampleRequests};

/// The owner of each line; the discriminant is its FROM_CPU line number.
#[repr(u8)]
pub(crate) enum Line {
    /// Core 0's executor wake.
    Executor0 = 0,
    /// Core 1's executor wake.
    #[cfg_attr(not(feature = "open-radio-hil"), allow(dead_code))]
    Executor1 = 1,
    /// Core 1's sample, for the hang watchdog and the program-counter
    /// profiler.
    #[cfg_attr(
        not(all(feature = "open-radio-hil", not(feature = "memory-benchmark"))),
        allow(dead_code)
    )]
    Core1Sample = 2,
    /// The IEEE 802.15.4 doorbell (#668); until it is bound, the
    /// interrupt-latency probe (#643).
    #[cfg_attr(not(feature = "interrupt-latency-probe"), allow(dead_code))]
    RadioDoorbell = 3,
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

/// The pending requests for core 1's sample.
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
static CORE1_SAMPLE_REQUESTS: SampleRequests = SampleRequests::new();

/// Core 1's sample line: core 0's raise and core 1's reset.
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
fn core1_sample() -> SoftwareInterrupt<'static, { Line::Core1Sample as u8 }> {
    // SAFETY: the line has one owner, core 1's sample; raising and
    // resetting touch only this line.
    #[allow(unsafe_code, reason = "the line is shared by two cores")]
    SoftwareInterrupt::new(unsafe { esp_hal::peripherals::FROM_CPU_INTR2::steal() })
}

/// Enable core 1's sample; call on core 1 before its executor enables
/// interrupts. `token` is its source's, whose table entry names
/// [`sample_core1`] on core 1.
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
pub(crate) fn enable_core1_sample(token: crate::Core1Sample) {
    if let Err(error) = oer_espressif_interrupt_table_esp_hal::enable(&token) {
        panic!("core 1 sample: {error:?}");
    }
}

/// Ask core 1 for a sample on behalf of `requester`.
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
pub(crate) fn request_core1_sample(requester: Requester) {
    CORE1_SAMPLE_REQUESTS.request(requester);
    core1_sample().raise();
}

/// An interrupt-table handler of the image: core 1's sample, serving every
/// pending requester.
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub(crate) fn sample_core1() {
    // Reset before taking: a request set after the reset raises the line
    // again, so no flag is left without a raise.
    core1_sample().reset();
    let pending = CORE1_SAMPLE_REQUESTS.take();
    if pending.contains(Requester::HangWatchdog) {
        crate::hang_watchdog::record_core1_context();
    }
    #[cfg(feature = "pc-profile")]
    if pending.contains(Requester::Profiler) {
        crate::pc_profile::record_core1();
    }
}

/// The interrupt-latency probe's line (#643): the IEEE 802.15.4 doorbell's,
/// which no image binds yet (#668).
#[cfg(feature = "interrupt-latency-probe")]
pub(crate) fn interrupt_latency_probe() -> SoftwareInterrupt<'static, { Line::RadioDoorbell as u8 }>
{
    // SAFETY: the line has one owner in this image, the probe; raising and
    // resetting touch only this line.
    #[allow(
        unsafe_code,
        reason = "the raise and the handler's reset share the line"
    )]
    SoftwareInterrupt::new(unsafe { esp_hal::peripherals::FROM_CPU_INTR3::steal() })
}
