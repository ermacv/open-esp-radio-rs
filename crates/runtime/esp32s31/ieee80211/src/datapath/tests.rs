use embassy_time::Instant;

use super::{TX_BATCH_MAX_WAIT, TxBatchDemand, TxBatchState};

#[test]
fn a_complete_demand_publishes_without_collection_inside_a_burst() {
    let mut state = TxBatchState::new();
    let now = Instant::from_micros(1_000);
    state.note_started(2);
    let collecting = TxBatchDemand {
        target: 32,
        ready: 1,
    };
    assert_eq!(
        state.collection_deadline(collecting, now),
        Some(now + TX_BATCH_MAX_WAIT)
    );
    assert_eq!(
        state.collection_deadline(TxBatchDemand::single(1), now),
        None,
        "a single-MPDU destination never waits for another peer's aggregate"
    );
    assert_eq!(
        state.collection_deadline(
            TxBatchDemand {
                target: 32,
                ready: 32
            },
            now
        ),
        None
    );
}
