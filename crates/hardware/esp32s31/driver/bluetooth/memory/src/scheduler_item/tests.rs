use core::num::NonZeroU32;

use vcell::VolatileCell;

use super::{SchedulerItemCompletionStatus, SchedulerItemHeader};
use crate::sram_link::ControllerSramLinkAddress;

fn item() -> [VolatileCell<u32>; 0x60 / 4] {
    [const { VolatileCell::new(0) }; 0x60 / 4]
}

#[test]
fn linking_preserves_the_role_allocation_bits() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.set_hardware_next_word(0x0030_0000);
    let next = ControllerSramLinkAddress::new(0x2f00_0100).expect("test link is representable");

    header.link_hardware_next(Some(next));
    assert_eq!(
        header.hardware_next_word(),
        0x0030_0000 | next.compressed_image()
    );
    assert_eq!(header.hardware_next_image(), next.compressed_image());

    header.link_hardware_next(None);
    assert_eq!(header.hardware_next_word(), 0x0030_0000);
    assert_eq!(header.hardware_next_image(), 0);
}

#[test]
fn the_sequence_adds_the_lead_and_keeps_the_window_duration() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.set_sequence(0xffff_fff0, 0x0000_0010, 4);
    assert_eq!(header.sequence_start(), 0xffff_fff4);
    assert_eq!(header.sequence_duration(), 0x20);
    assert_eq!(words[0x0c / 4].get(), 0xffff_fff4);
    assert_eq!(words[0x10 / 4].get(), 0x20);
}

#[test]
fn control_bytes_are_cleared_independently() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.set_control(0x1122_3344);

    header.clear_event_byte();
    assert_eq!(header.control(), 0x1122_3300);
    header.clear_scheduler_byte();
    assert_eq!(header.control(), 0x1100_3300);
}

#[test]
fn status_decodes_the_unexecuted_sentinel_zero_and_nonzero() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.mark_unexecuted();
    assert_eq!(words[0x38 / 4].get(), u32::MAX);
    assert_eq!(header.completion_status(), None);

    header.set_status(0);
    assert_eq!(
        header.completion_status(),
        Some(SchedulerItemCompletionStatus::Zero)
    );
    header.set_status(5);
    assert_eq!(
        header.completion_status(),
        Some(SchedulerItemCompletionStatus::NonZero(
            NonZeroU32::new(5).expect("test status is nonzero")
        ))
    );
}

#[test]
fn window_and_links_use_their_own_words() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.set_raw_start(10);
    header.set_raw_end(20);
    header.set_previous(0x2f00_0200);
    header.set_completion_link(0x2f00_0300);

    assert_eq!((header.raw_start(), header.raw_end()), (10, 20));
    assert_eq!((words[0x44 / 4].get(), words[0x48 / 4].get()), (10, 20));
    assert_eq!(words[0x50 / 4].get(), 0x2f00_0200);
    assert_eq!(words[0x54 / 4].get(), 0x2f00_0300);
    assert_eq!(header.previous(), 0x2f00_0200);
    assert_eq!(header.completion_link(), 0x2f00_0300);
}
