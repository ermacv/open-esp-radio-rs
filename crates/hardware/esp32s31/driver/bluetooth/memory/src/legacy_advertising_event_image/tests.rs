use super::{
    LegacyAdvertisingLinkStateWords, LegacyAdvertisingOwnAddress, LegacyAdvertisingPrimaryChannel,
    LegacyAdvertisingSchedulerItemWords,
};
use crate::{ControllerSramLinkAddress, LeTxPower};

fn zero_link_state() -> LegacyAdvertisingLinkStateWords {
    LegacyAdvertisingLinkStateWords {
        word_00: 0,
        word_04: 0,
        word_08: 0,
        word_0c: 0,
        word_14: 0,
        word_18: 0,
        word_24: 0,
        crc_init_word_2c: 0,
        word_30: 0,
        word_34: 0,
        access_address_word_38: 0,
        word_3c: 0,
        word_40: 0,
        word_50: 0,
        word_60: 0,
    }
}

fn zero_item() -> LegacyAdvertisingSchedulerItemWords {
    LegacyAdvertisingSchedulerItemWords {
        word_00: 0,
        word_04: 0,
        word_14: 0,
        word_18: 0,
        word_38: 0,
        raw_start_word_44: 0,
        raw_end_word_48: 0,
        word_4c: 0,
    }
}

fn reset(dbm: i8) -> LegacyAdvertisingLinkStateWords {
    zero_link_state().reset(
        ControllerSramLinkAddress::new(0x2f00_1000).expect("controller SRAM"),
        LegacyAdvertisingOwnAddress::Public,
        LeTxPower::from_dbm(dbm).expect("provider level"),
    )
}

#[test]
fn an_item_carries_the_power_its_link_state_was_reset_with() {
    for dbm in [-24, 0, 9, 21] {
        let link_state = reset(dbm);
        assert_eq!(
            link_state.power_index(),
            u32::from(LeTxPower::from_dbm(dbm).unwrap().index())
        );
        let item = zero_item().prepare_event_item(
            link_state,
            LegacyAdvertisingPrimaryChannel::Channel37,
            None,
            10,
            20,
        );
        assert_eq!(item.word_14 >> 20 & 0xff, link_state.power_index());
    }
}

#[test]
fn the_power_leaves_the_rate_word_untouched() {
    let low = reset(-24);
    let high = reset(21);
    assert_eq!(low.word_04, high.word_04);
    // Only the power byte of the priority word differs.
    assert_eq!(low.word_60 & 0xff, high.word_60 & 0xff);
    assert_ne!(low.word_60, high.word_60);
}
