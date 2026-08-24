//! Closed IEEE 802.15.4 `EVENT_STATUS` validation transaction.
//!
//! This module is compiled only into validation images.  In particular, it
//! does not add [`crate::Writable`] to the generated read-only
//! `EVENT_STATUS` register.  The only raw writes name timer-zero or timer-one
//! independently so a HIL probe can test the unresolved acknowledge access
//! class for selective-W1C compatibility without creating a general
//! acknowledge capability.

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
    // SAFETY: the generated accessor is intentionally read-only because its
    // modified-write class is unresolved.  This feature-only experiment must
    // bypass that access class to write exactly the independently stimulated
    // timer-zero bit.  The unique PAC lease prevents a concurrent task-side
    // register owner.  The closed HAL transaction verifies EVENT_ENABLE; its
    // isolated HIL caller must also keep the external CPU route unconfigured.
    unsafe {
        core::ptr::write_volatile(registers.event_status().as_ptr(), TIMER0_EVENT);
    }
    order_device_accesses();
}

/// Perform the experiment's raw write of only timer-one's event bit.
#[inline]
pub fn write_timer1_event(registers: &crate::Ieee802154Mac) {
    // SAFETY: see [`write_timer0_event`].  The only difference is the
    // independently stimulated timer-one bit.
    unsafe {
        core::ptr::write_volatile(registers.event_status().as_ptr(), TIMER1_EVENT);
    }
    order_device_accesses();
}

#[cfg(test)]
mod tests {
    use super::{EVENT_FIELD_MASK, TIMER_EVENTS, replace_event_field};

    #[test]
    fn event_field_replacement_preserves_every_unowned_upper_bit() {
        let sentinel = 0xa5a5_c000;
        assert_eq!(
            replace_event_field(sentinel, TIMER_EVENTS),
            sentinel | 0x0300
        );
        assert_eq!(
            replace_event_field(sentinel | EVENT_FIELD_MASK, 0),
            sentinel
        );
    }
}
