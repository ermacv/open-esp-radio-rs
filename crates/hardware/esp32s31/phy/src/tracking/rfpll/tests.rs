use super::{Action, Completion, Correction, search};
use crate::analog::frequency::{
    PhyFrequencyCapMemoryAction as MemoryAction,
    PhyFrequencyCapMemoryCompletion as MemoryCompletion,
};
use search::{
    Action as SearchAction, Completion as SearchCompletion, SAMPLES_PER_DIRECTION, Status,
};

const DIRECTION: usize = SAMPLES_PER_DIRECTION as usize;

fn complete_search_action(action: SearchAction, initial: u16, status: Status) -> SearchCompletion {
    match action {
        SearchAction::ReadInitialCap => SearchCompletion::InitialCap(initial),
        SearchAction::EnableSearch => SearchCompletion::SearchEnabled,
        SearchAction::WriteCap(cap) => SearchCompletion::CapWritten(cap),
        SearchAction::DelayMicros(micros) => SearchCompletion::DelayElapsed(micros),
        SearchAction::ReadStatus => SearchCompletion::Status(status),
        SearchAction::Complete(_) => panic!("search already complete"),
    }
}

fn run_search(initial: u16, statuses: &[Status]) -> (search::Outcome, std::vec::Vec<i16>) {
    let mut search = search::Search::new();
    let mut samples = 0;
    let mut writes = std::vec::Vec::new();
    let mut settles = 0;
    for _ in 0..8 * DIRECTION {
        let action = search.action();
        if let SearchAction::Complete(outcome) = action {
            assert_eq!(samples, statuses.len());
            assert_eq!(settles, samples);
            assert_eq!(
                writes.len(),
                samples + 1,
                "final capacitor is programmed even without accepted samples"
            );
            return (outcome, writes);
        }
        let mut status = Status::Other;
        if action == SearchAction::ReadStatus {
            status = statuses[samples];
            samples += 1;
        }
        if let SearchAction::WriteCap(cap) = action {
            writes.push(cap);
        }
        if action == SearchAction::DelayMicros(5) {
            settles += 1;
        }
        search
            .advance(complete_search_action(action, initial, status))
            .unwrap();
    }
    panic!("finite search did not terminate");
}

fn mean(candidates: &[i16]) -> i16 {
    candidates.iter().sum::<i16>() / candidates.len() as i16
}

#[test]
fn measured_correction_is_not_limited_to_two() {
    let mut statuses = [Status::Accepted; DIRECTION + 2];
    statuses[..2].fill(Status::Increase);
    let (outcome, writes) = run_search(100, &statuses);
    assert_eq!(outcome.accepted_samples, SAMPLES_PER_DIRECTION);
    let upward = (101..).take(DIRECTION).collect::<std::vec::Vec<i16>>();
    assert_eq!(outcome.delta(), mean(&upward) - 100);
    assert_eq!(&writes[..2], &[100, 99]);
    assert_eq!(&writes[2..2 + DIRECTION], upward.as_slice());
    assert_eq!(writes.last(), Some(&(100 + outcome.delta())));

    statuses.fill(Status::Accepted);
    statuses[DIRECTION..].fill(Status::Decrease);
    let (outcome, _) = run_search(100, &statuses);
    let downward = (0..DIRECTION as i16).map(|offset| 100 - offset);
    assert_eq!(
        outcome.delta(),
        mean(&downward.collect::<std::vec::Vec<_>>()) - 100
    );
}

#[test]
fn direction_boundaries_need_not_be_consecutive() {
    let (outcome, writes) = run_search(
        100,
        &[
            Status::Increase,
            Status::Accepted,
            Status::Increase,
            Status::Decrease,
            Status::Other,
            Status::Decrease,
        ],
    );
    assert_eq!(outcome.delta(), -1);
    assert_eq!(outcome.accepted_samples, 1);
    assert_eq!(writes, [100, 99, 98, 101, 102, 103, 99]);
}

#[test]
fn no_accepted_samples_preserves_initial_cap_after_bounded_search() {
    for status in [Status::Other, Status::Accepted] {
        let (outcome, _) = run_search(100, &[status; 2 * DIRECTION]);
        assert_eq!(outcome.delta(), 0);
    }
    let (outcome, writes) = run_search(
        100,
        &[
            Status::Increase,
            Status::Increase,
            Status::Decrease,
            Status::Decrease,
        ],
    );
    assert_eq!(outcome.selected_cap, 100);
    assert_eq!(writes, [100, 99, 101, 102, 100]);
}

#[test]
fn opposite_direction_status_does_not_terminate_a_phase() {
    let mut statuses = [Status::Decrease; 2 * DIRECTION];
    statuses[DIRECTION..].fill(Status::Increase);
    let (outcome, writes) = run_search(100, &statuses);
    assert_eq!(outcome.accepted_samples, 0);
    assert_eq!(writes.len(), 2 * DIRECTION + 1);
}

#[test]
fn search_preserves_signed_candidate_and_wrapping_accumulation() {
    let (outcome, writes) = run_search(0, &[Status::Accepted; 2 * DIRECTION]);
    assert_eq!(&writes[..3], &[0, -1, -2]);
    assert_eq!(outcome.selected_cap, 0);
    // Vendor accumulates the requested u16 candidate, even when the helper
    // clamps the signed write to zero. It does not average the clamped value.
    let (outcome, _) = run_search(
        0,
        &[
            Status::Increase,
            Status::Accepted,
            Status::Increase,
            Status::Decrease,
            Status::Decrease,
        ],
    );
    assert_eq!(outcome.selected_cap, u16::MAX);
    assert_eq!(outcome.delta(), -1);
}

#[test]
fn stale_or_misordered_completion_leaves_search_unchanged() {
    let mut search = search::Search::new();
    assert_eq!(
        search.advance(SearchCompletion::SearchEnabled),
        Err(search::Error::WrongCompletion)
    );
    assert_eq!(search.action(), SearchAction::ReadInitialCap);
    search.advance(SearchCompletion::InitialCap(100)).unwrap();
    search.advance(SearchCompletion::SearchEnabled).unwrap();
    assert_eq!(
        search.advance(SearchCompletion::CapWritten(99)),
        Err(search::Error::WrongCompletion)
    );
    assert_eq!(search.action(), SearchAction::WriteCap(100));
    search.advance(SearchCompletion::CapWritten(100)).unwrap();
    assert_eq!(
        search.advance(SearchCompletion::DelayElapsed(4)),
        Err(search::Error::WrongCompletion)
    );
    assert_eq!(search.action(), SearchAction::DelayMicros(5));
}

#[test]
fn correction_updates_memory_only_for_nonzero_delta_and_waits_for_restore() {
    for measured in [false, true] {
        let mut correction = Correction::new(13);
        let mut samples = 0;
        let mut memory_writes = 0;
        let mut restored = false;
        let outcome = loop {
            let completion = match correction.action() {
                Action::Search(action) => {
                    let status = if measured && samples < 2 {
                        Status::Increase
                    } else {
                        Status::Accepted
                    };
                    if action == SearchAction::ReadStatus {
                        samples += 1;
                    }
                    Completion::Search(complete_search_action(action, 100, status))
                }
                Action::Memory(MemoryAction::ReadMemory {
                    entry_index,
                    address,
                    mode,
                }) => Completion::Memory(MemoryCompletion::MemoryRead {
                    entry_index,
                    address,
                    mode,
                    value: 100,
                }),
                Action::Memory(MemoryAction::WriteMemory {
                    entry_index,
                    address,
                    mode,
                    value,
                }) => {
                    assert!(measured);
                    let upward = (101..).take(DIRECTION).collect::<std::vec::Vec<i16>>();
                    assert_eq!(
                        value as i16,
                        mean(&upward),
                        "every entry receives the full measured correction"
                    );
                    memory_writes += 1;
                    Completion::Memory(MemoryCompletion::MemoryWritten {
                        entry_index,
                        address,
                        mode,
                    })
                }
                Action::Memory(MemoryAction::RestoreChannelIndex { frequency_index }) => {
                    assert_eq!(memory_writes, 85);
                    assert!(!restored);
                    restored = true;
                    Completion::Memory(MemoryCompletion::ChannelIndexRestored { frequency_index })
                }
                Action::Complete(outcome) => break outcome,
                Action::Memory(MemoryAction::Complete(_)) => {
                    panic!("child completion must be consumed by its owner")
                }
            };
            correction.advance(completion).unwrap();
        };
        assert_eq!(outcome.memory.is_some(), measured);
        assert_eq!(restored, measured);
        assert_eq!(memory_writes, if measured { 85 } else { 0 });
        assert_eq!(
            correction.advance(Completion::Search(SearchCompletion::SearchEnabled)),
            Err(super::Error::AlreadyComplete)
        );
    }
}
