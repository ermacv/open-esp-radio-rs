//! Closed IEEE 802.15.4 `EVENT_STATUS` validation transaction.
//!
//! This module is compiled only into validation images. Production code uses
//! the generated affine W1C snapshot. The literal timer-zero and timer-one
//! writes remain isolated here solely to preserve the historical selective
//! clearing discriminator; neither accepts a caller-provided image.

const TIMER0_EVENT: u32 = 1 << 8;
const TIMER1_EVENT: u32 = 1 << 9;
const TIMER_EVENTS: u16 = (TIMER0_EVENT | TIMER1_EVENT) as u16;
const EVENT_FIELD_MASK: u32 = 0x3fff;

const fn replace_event_field(current: u32, events: u16) -> u32 {
    (current & !EVENT_FIELD_MASK) | ((events as u32) & EVENT_FIELD_MASK)
}

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
    // SAFETY: the raw replacement changes only the reviewed fourteen-bit RW
    // field and preserves every unowned upper bit from the fresh register
    // read. The closed transaction selects exactly the two timer events.
    registers.event_enable().modify(|reader, writer| unsafe {
        writer.bits(replace_event_field(reader.bits(), TIMER_EVENTS))
    });
    order_device_accesses();
}

/// Replace only the event-enable field with zero during cleanup.
#[inline]
pub fn disable_all_events(registers: &crate::Ieee802154Mac) {
    // SAFETY: see [`enable_timer_events`]. The reviewed field becomes the
    // complete masked image while every unowned upper bit is retained.
    registers
        .event_enable()
        .modify(|reader, writer| unsafe { writer.bits(replace_event_field(reader.bits(), 0)) });
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

/// Perform the experiment's raw write of only timer-zero's event bit.
#[inline]
pub fn write_timer0_event(registers: &crate::Ieee802154Mac) {
    // SAFETY: the feature-only experiment writes exactly one fixed event bit.
    // The unique PAC lease prevents a concurrent task-side register owner and
    // the isolated caller keeps the external CPU route unconfigured.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.bits(TIMER0_EVENT));
    }
    order_device_accesses();
}

/// Perform the experiment's raw write of only timer-one's event bit.
#[inline]
pub fn write_timer1_event(registers: &crate::Ieee802154Mac) {
    // SAFETY: see [`write_timer0_event`].  The only difference is the
    // independently stimulated timer-one bit.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.bits(TIMER1_EVENT));
    }
    order_device_accesses();
}
