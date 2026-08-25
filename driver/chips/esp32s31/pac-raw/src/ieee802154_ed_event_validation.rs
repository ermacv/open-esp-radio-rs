//! Closed ED-DONE/TIMER0 `EVENT_STATUS` validation transaction vocabulary.
//!
//! This module is intended only for a reset-isolated validation image. The
//! production API uses the generated affine W1C snapshot; these two literal
//! writes remain feature-gated so the historical discriminator can compare
//! ED-DONE and TIMER0 independently without exposing a general raw image.

const RX_ABORT_EVENT: u32 = 1 << 4;
const ED_DONE_EVENT: u32 = 1 << 6;
const TIMER0_EVENT: u32 = 1 << 8;
#[cfg(test)]
const ED_TIMER_EVENTS: u16 = (ED_DONE_EVENT | TIMER0_EVENT) as u16;
const VALIDATION_EVENTS: u16 = (RX_ABORT_EVENT | ED_DONE_EVENT | TIMER0_EVENT) as u16;
const ED_ABORT_REASONS: u32 = (1 << 23) | (1 << 24) | (1 << 25);
const EVENT_FIELD_MASK: u32 = 0x3fff;
const RX_ABORT_FIELD_MASK: u32 = 0x7fff_ffff;

/// Fixed public-LL energy-detection duration used by the discriminator.
pub const VALIDATION_ED_DURATION: u32 = 8;

const fn replace_event_field(current: u32, events: u16) -> u32 {
    (current & !EVENT_FIELD_MASK) | ((events as u32) & EVENT_FIELD_MASK)
}

const fn replace_rx_abort_field(current: u32, reasons: u32) -> u32 {
    (current & !RX_ABORT_FIELD_MASK) | (reasons & RX_ABORT_FIELD_MASK)
}

#[inline]
fn order_device_accesses() {
    crate::device_access::fence();
}

/// Return the complete fourteen-bit event-delivery mask without changing it.
#[inline]
pub fn event_enable_events(registers: &crate::Ieee802154Mac) -> u16 {
    order_device_accesses();
    let events = registers.event_enable().read().events().bits();
    order_device_accesses();
    events
}

/// Replace only the event-enable field with RX-ABORT, ED-DONE and TIMER0.
#[inline]
pub fn enable_ed_timer_abort_events(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: the raw replacement changes only the reviewed fourteen-bit RW
    // field and retains every unowned upper bit from the fresh register read.
    // RX-ABORT is enabled so missing RF/BTBB readiness cannot turn a terminal
    // ED abort into a silent timeout. The selective-write pair remains only
    // ED-DONE and TIMER0.
    registers.event_enable().modify(|reader, writer| unsafe {
        writer.bits(replace_event_field(reader.bits(), VALIDATION_EVENTS))
    });
    order_device_accesses();
}

/// Replace only the event-enable field with zero during cleanup.
#[inline]
pub fn disable_all_events(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: see [`enable_ed_timer_abort_events`]. The owned field becomes
    // zero and every unowned upper bit is retained from the fresh read.
    registers
        .event_enable()
        .modify(|reader, writer| unsafe { writer.bits(replace_event_field(reader.bits(), 0)) });
    order_device_accesses();
}

/// Return one complete fourteen-bit `EVENT_STATUS` sample.
#[inline]
pub fn event_status_events(registers: &crate::Ieee802154Mac) -> u16 {
    order_device_accesses();
    let events = registers.event_status().read().events().bits();
    order_device_accesses();
    events
}

/// Return the complete raw RX status for fail-closed abort diagnosis.
#[inline]
pub fn rx_status_raw(registers: &crate::Ieee802154Mac) -> u32 {
    order_device_accesses();
    let status = registers.rx_status().read().bits();
    order_device_accesses();
    status
}

/// Return the complete thirty-one-bit RX-abort delivery mask.
#[inline]
pub fn rx_abort_enable_events(registers: &crate::Ieee802154Mac) -> u32 {
    order_device_accesses();
    let events = registers.rx_abort_enable().read().events().bits();
    order_device_accesses();
    events
}

/// Select exactly ED_ABORT, ED_STOP and ED_COEX_REJECT abort reasons.
#[inline]
pub fn enable_ed_abort_reasons(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: the raw replacement changes only the reviewed low thirty-one RW
    // bits, preserves the unowned high bit, and publishes exactly the three
    // source-confirmed ED terminal reasons.
    registers.rx_abort_enable().modify(|reader, writer| unsafe {
        writer.bits(replace_rx_abort_field(reader.bits(), ED_ABORT_REASONS))
    });
    order_device_accesses();
}

/// Mask every RX-abort reason during terminal cleanup.
#[inline]
pub fn disable_all_rx_abort_reasons(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: see [`enable_ed_abort_reasons`]. The owned field becomes zero;
    // no arbitrary caller-provided register image crosses this raw boundary.
    registers
        .rx_abort_enable()
        .modify(|reader, writer| unsafe { writer.bits(replace_rx_abort_field(reader.bits(), 0)) });
    order_device_accesses();
}

/// Return the complete public energy-detection duration field.
#[inline]
pub fn ed_duration(registers: &crate::Ieee802154Mac) -> u32 {
    order_device_accesses();
    let duration = registers.ed_duration().read().duration().bits();
    order_device_accesses();
    duration
}

/// Program the public standalone-CCA energy-detection duration, exactly eight.
#[inline]
pub fn set_ed_duration_eight(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: the generated field is the reviewed low twenty-four-bit RW
    // duration. RMW matches the public bitfield assignment and retains the
    // adjacent high byte. This closed API admits only the fixed public-LL
    // standalone-CCA value eight.
    registers
        .ed_duration()
        .modify(|_reader, writer| unsafe { writer.duration().bits(VALIDATION_ED_DURATION) });
    order_device_accesses();
}

/// Return one complete TIMER0 counter sample.
#[inline]
pub fn timer0_value(registers: &crate::Ieee802154Mac) -> u32 {
    order_device_accesses();
    let value = registers.timer0_value().read().value().bits();
    order_device_accesses();
    value
}

/// Program the validation timer with one nonzero field-sized threshold.
#[inline]
pub fn set_timer0_threshold(registers: &mut crate::Ieee802154Mac, threshold: u32) {
    // SAFETY: the generated threshold register is a complete 32-bit
    // write-only word. The validation HAL admits only a nonzero bounded value.
    unsafe {
        registers
            .timer0_threshold()
            .write_with_zero(|writer| writer.threshold().bits(threshold));
    }
    order_device_accesses();
}

/// Start TIMER0 through its source-confirmed finite opcode.
#[inline]
pub fn start_timer0(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: the generated field admits this public-LL opcode and the command
    // is issued only inside the closed validation transaction.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_start());
    }
    order_device_accesses();
}

/// Stop TIMER0 through its source-confirmed finite opcode.
#[inline]
pub fn stop_timer0(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: see [`start_timer0`]; this is the paired finite stop opcode.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_stop());
    }
    order_device_accesses();
}

/// Start the fixed-duration energy-detection stimulus.
#[inline]
pub fn start_ed(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: the generated field admits the public-LL ED_START opcode. This
    // module does not claim that PHY/RF/BTBB prerequisites are ready; any abort
    // remains visible in the complete status evidence and fails the probe.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().ed_start());
    }
    order_device_accesses();
}

/// Issue the public-LL STOP opcode as best-effort timeout cleanup.
#[inline]
pub fn stop_operation(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: this is the finite public-LL STOP opcode. It is cleanup for a
    // validation timeout, not evidence that STOP is synchronous.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().stop());
    }
    order_device_accesses();
}

/// Perform the discriminator's raw write of only ED-DONE.
#[inline]
pub fn write_ed_done_event(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: this reset-isolated validation experiment publishes one fixed
    // source-confirmed event bit. Production acknowledgement goes through the
    // generated affine W1C snapshot and cannot call this feature-only leaf.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.bits(ED_DONE_EVENT));
    }
    order_device_accesses();
}

/// Perform the discriminator's raw write of only TIMER0.
#[inline]
pub fn write_timer0_event(registers: &mut crate::Ieee802154Mac) {
    // SAFETY: as above, the validation image publishes one fixed event bit and
    // retains a unique PAC lease with the CPU route detached.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.bits(TIMER0_EVENT));
    }
    order_device_accesses();
}

#[cfg(test)]
mod tests {
    use super::{
        ED_ABORT_REASONS, ED_TIMER_EVENTS, EVENT_FIELD_MASK, RX_ABORT_FIELD_MASK,
        VALIDATION_ED_DURATION, VALIDATION_EVENTS, replace_event_field, replace_rx_abort_field,
    };

    #[test]
    fn fixed_validation_image_preserves_every_unowned_upper_bit() {
        let sentinel = 0xa5a5_c000;
        assert_eq!(ED_TIMER_EVENTS, 0x0140);
        assert_eq!(VALIDATION_EVENTS, 0x0150);
        assert_eq!(ED_ABORT_REASONS, 0x0380_0000);
        assert_eq!(VALIDATION_ED_DURATION, 8);
        assert_eq!(
            replace_event_field(sentinel, VALIDATION_EVENTS),
            sentinel | 0x0150
        );
        assert_eq!(
            replace_event_field(sentinel | EVENT_FIELD_MASK, 0),
            sentinel
        );
        assert_eq!(
            replace_rx_abort_field(0x8000_0000, ED_ABORT_REASONS),
            0x8380_0000
        );
        assert_eq!(replace_rx_abort_field(RX_ABORT_FIELD_MASK, 0), 0);
    }
}
