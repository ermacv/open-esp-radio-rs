//! Closed IEEE 802.15.4 `EVENT_STATUS` validation transaction.
//!
//! This module is compiled only into validation images. Production code uses
//! the generated affine W1C snapshot. The named timer-zero and timer-one
//! accessors remain isolated here solely to preserve the historical selective
//! clearing discriminator; neither accepts a caller-provided image.

#[inline]
fn order_device_accesses() {
    crate::device_access::fence();
}

/// Return the interrupt-delivery mask without changing it.
#[inline]
pub fn event_enable_events(registers: &crate::Ieee802154Mac) -> u16 {
    order_device_accesses();
    registers.event_enable().read().events().bits()
}

/// Replace only the event-enable field with the two validation timers.
#[inline]
pub fn enable_timer_events(registers: &crate::Ieee802154Mac) {
    registers
        .event_enable()
        .modify(|_, writer| writer.events().timer_pair_validation());
    order_device_accesses();
}

/// Replace only the event-enable field with zero during cleanup.
#[inline]
pub fn disable_all_events(registers: &crate::Ieee802154Mac) {
    registers
        .event_enable()
        .modify(|_, writer| writer.events().none());
    order_device_accesses();
}

/// Return one raw fourteen-bit `EVENT_STATUS` sample.
#[inline]
pub fn event_status_events(registers: &crate::Ieee802154Mac) -> u16 {
    order_device_accesses();
    registers.event_status().read().events().bits()
}

/// Return one complete timer-zero counter sample.
#[inline]
pub fn timer0_value(registers: &crate::Ieee802154Mac) -> u32 {
    order_device_accesses();
    registers.timer0_value().read().value().bits()
}

/// Return one complete timer-one counter sample.
#[inline]
pub fn timer1_value(registers: &crate::Ieee802154Mac) -> u32 {
    order_device_accesses();
    registers.timer1_value().read().value().bits()
}

/// Program both validation timers with the same nonzero threshold.
#[inline]
pub fn set_timer_thresholds(registers: &crate::Ieee802154Mac, threshold: u32) {
    // SAFETY: both generated registers are complete 32-bit write-only words.
    // The validation HAL admits only a nonzero field-sized threshold.
    unsafe {
        registers
            .timer0_threshold()
            .write_with_zero(|writer| writer.threshold().bits(threshold));
        registers
            .timer1_threshold()
            .write_with_zero(|writer| writer.threshold().bits(threshold));
    }
    order_device_accesses();
}

/// Start validation timer zero through its source-confirmed opcode.
#[inline]
pub fn start_timer0(registers: &crate::Ieee802154Mac) {
    // SAFETY: the generated field admits this finite public-LL opcode and the
    // command is issued only by the closed validation transaction.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_start());
    }
    order_device_accesses();
}

/// Stop validation timer zero through its source-confirmed opcode.
#[inline]
pub fn stop_timer0(registers: &crate::Ieee802154Mac) {
    // SAFETY: see [`start_timer0`]; this is the paired finite stop opcode.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_stop());
    }
    order_device_accesses();
}

/// Start validation timer one through its source-confirmed opcode.
#[inline]
pub fn start_timer1(registers: &crate::Ieee802154Mac) {
    // SAFETY: the generated field admits this finite public-LL opcode and the
    // command is issued only by the closed validation transaction.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer1_start());
    }
    order_device_accesses();
}

/// Stop validation timer one through its source-confirmed opcode.
#[inline]
pub fn stop_timer1(registers: &crate::Ieee802154Mac) {
    // SAFETY: see [`start_timer1`]; this is the paired finite stop opcode.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer1_stop());
    }
    order_device_accesses();
}

/// Select only timer zero through its generated W1C field variant.
#[inline]
pub fn write_timer0_event(registers: &crate::Ieee802154Mac) {
    // SAFETY: the generated write-only variant selects only TIMER0 in this
    // reset-isolated validation transaction; no integer image is accepted.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.events().timer0_only());
    }
    order_device_accesses();
}

/// Select only timer one through its generated W1C field variant.
#[inline]
pub fn write_timer1_event(registers: &crate::Ieee802154Mac) {
    // SAFETY: the generated write-only variant selects only TIMER1 in this
    // reset-isolated validation transaction; no integer image is accepted.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.events().timer1_only());
    }
    order_device_accesses();
}
