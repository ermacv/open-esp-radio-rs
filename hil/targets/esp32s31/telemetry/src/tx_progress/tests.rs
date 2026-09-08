use super::*;

#[test]
fn blocked_socket_and_downstream_gap_are_distinct() {
    let socket = Counters::new();
    let radio = Counters::new();
    socket.admitted(0, 100);
    radio.admitted(0, 110);
    socket.blocked(120);
    socket.blocked(130);
    assert_eq!(socket.snapshot(140).pending_micros, 20);
    socket.admitted(1, 160);
    socket.admitted(2, 210);
    radio.admitted(1, 1_000);
    assert_eq!(socket.snapshot(220).maximum_wait_micros, 40);
    assert_eq!(socket.snapshot(220).maximum_gap_micros, 60);
    assert_eq!(radio.snapshot(1_010).maximum_gap_micros, 890);
    assert_eq!(radio.snapshot(1_010).sequence_after_maximum_gap, 1);
    assert_eq!(radio.snapshot(1_010).count, 2);
}

#[test]
fn clock_wrap_and_new_session_do_not_manufacture_a_gap() {
    let counter = Counters::new();
    counter.admitted(0, u32::MAX - 9);
    counter.admitted(1, 10);
    assert_eq!(counter.snapshot(20).maximum_gap_micros, 20);
    counter.reset();
    assert_eq!(counter.snapshot(100), Snapshot::default());
    counter.admitted(0, 200);
    assert_eq!(counter.snapshot(210).maximum_gap_micros, 0);
}
