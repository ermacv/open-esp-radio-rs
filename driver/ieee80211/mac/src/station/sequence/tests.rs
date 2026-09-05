use super::*;

#[test]
fn sta_sequence_counter_is_monotonic_across_twelve_bit_wrap() {
    let mut sequence = StaSequenceCounter::new(0x1ffe);
    assert_eq!(sequence.take(), 0x0ffe);
    assert_eq!(sequence.take(), 0x0fff);
    assert_eq!(sequence.take(), 0x0000);
    assert_eq!(sequence.peek(), 0x0001);
}

#[test]
fn sta_tx_sequence_spaces_do_not_advance_each_other() {
    let mut sequences = StaTxSequenceCounters::new(25);

    assert_eq!(sequences.take_non_qos(), 25);
    assert_eq!(sequences.take_non_qos(), 26);
    assert_eq!(sequences.peek_qos(0), Some(25));
    assert_eq!(sequences.peek_qos(5), Some(25));
    assert_eq!(sequences.peek_qos(7), Some(25));

    assert_eq!(sequences.take_qos(0), Some(25));
    assert_eq!(sequences.peek_qos(0), Some(26));
    assert_eq!(sequences.peek_qos(5), Some(25));
    assert_eq!(sequences.peek_qos(7), Some(25));
    assert_eq!(sequences.peek_non_qos(), 27);
}

#[test]
fn sta_tx_sequence_space_rejects_invalid_tid_and_wraps_independently() {
    let mut sequences = StaTxSequenceCounters::new(0x0fff);

    assert_eq!(sequences.take_data(Some(15)), Some(0x0fff));
    assert_eq!(sequences.peek_qos(15), Some(0));
    assert_eq!(sequences.take_data(None), Some(0x0fff));
    assert_eq!(sequences.peek_non_qos(), 0);
    assert_eq!(sequences.take_data(Some(16)), None);
}
