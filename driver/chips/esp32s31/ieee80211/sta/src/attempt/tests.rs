use core::future::ready;

use super::*;
use crate::test_support::block_on;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Owner {
    identity: u32,
    completed: u8,
    calls: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
    call_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Connected {
    identity: u32,
    calls: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Error(Esp32s31StaAttemptStage);

#[derive(Clone, Copy)]
struct MockPort {
    fail_at: Option<Esp32s31StaAttemptStage>,
}

impl MockPort {
    fn new(fail_at: Option<Esp32s31StaAttemptStage>) -> Self {
        Self { fail_at }
    }

    fn step(
        &mut self,
        owner: &mut Owner,
        stage: Esp32s31StaAttemptStage,
    ) -> Result<(), Esp32s31StaAttemptStepError<Error>> {
        owner.calls[owner.call_count] = Some(stage);
        owner.call_count += 1;
        if self.fail_at == Some(stage) {
            return Err(Esp32s31StaAttemptStepError::retry_current(Error(stage)));
        }
        owner.completed += 1;
        Ok(())
    }
}

impl Esp32s31StaAttemptPort for MockPort {
    type Owner = Owner;
    type Connected = Connected;
    type Error = Error;

    fn prepare_candidate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, Esp32s31StaAttemptStage::Candidate))
    }

    fn select_channel<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, Esp32s31StaAttemptStage::Channel))
    }

    fn authenticate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, Esp32s31StaAttemptStage::Authentication))
    }

    fn associate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, Esp32s31StaAttemptStage::Association))
    }

    fn program_peer<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, Esp32s31StaAttemptStage::PeerProgramming))
    }

    fn run_wpa2_handshake<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, Esp32s31StaAttemptStage::Wpa2Handshake))
    }

    fn install_wpa2_keys<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, Esp32s31StaAttemptStage::Wpa2KeyInstall))
    }

    fn enter_connected(
        &mut self,
        mut owner: Self::Owner,
    ) -> impl Future<
        Output = Result<
            Self::Connected,
            Esp32s31StaConnectedEntryFailure<Self::Owner, Self::Error>,
        >,
    > + '_ {
        owner.calls[owner.call_count] = Some(Esp32s31StaAttemptStage::ConnectedEntry);
        owner.call_count += 1;
        if self.fail_at == Some(Esp32s31StaAttemptStage::ConnectedEntry) {
            return ready(Err(Esp32s31StaConnectedEntryFailure::new(
                owner,
                StaFailureDisposition::Terminal,
                Error(Esp32s31StaAttemptStage::ConnectedEntry),
            )));
        }
        owner.completed += 1;
        ready(Ok(Connected {
            identity: owner.identity,
            calls: owner.calls,
        }))
    }
}

#[derive(Default)]
struct Observer {
    started: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
    completed: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
    failed: Option<(Esp32s31StaAttemptStage, StaFailureDisposition)>,
    started_count: usize,
    completed_count: usize,
}

impl Esp32s31StaAttemptObserver for Observer {
    fn stage_started(&mut self, stage: Esp32s31StaAttemptStage) {
        self.started[self.started_count] = Some(stage);
        self.started_count += 1;
    }

    fn stage_completed(&mut self, stage: Esp32s31StaAttemptStage) {
        self.completed[self.completed_count] = Some(stage);
        self.completed_count += 1;
    }

    fn stage_failed(&mut self, stage: Esp32s31StaAttemptStage, disposition: StaFailureDisposition) {
        self.failed = Some((stage, disposition));
    }
}

const STAGES: [Esp32s31StaAttemptStage; Esp32s31StaAttemptStage::COUNT as usize] = [
    Esp32s31StaAttemptStage::Candidate,
    Esp32s31StaAttemptStage::Channel,
    Esp32s31StaAttemptStage::Authentication,
    Esp32s31StaAttemptStage::Association,
    Esp32s31StaAttemptStage::PeerProgramming,
    Esp32s31StaAttemptStage::Wpa2Handshake,
    Esp32s31StaAttemptStage::Wpa2KeyInstall,
    Esp32s31StaAttemptStage::ConnectedEntry,
];

#[test]
fn selected_channel_preserves_negotiated_ht40_geometry() {
    let mut access_point = ScanRecord {
        channel: 6,
        ht_capability_ie_present: true,
        ht_operation_ie_present: true,
        ..ScanRecord::EMPTY
    };
    access_point.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
    access_point.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
    let station = Esp32s31StaAttemptStation {
        station_address: [0; 6],
        access_point,
        association_preference: StaAssociationPreference::Automatic,
        security: WifiSecurityMode::Open,
    };
    assert_eq!(
        station.selected_channel(),
        WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above)
    );
}

#[test]
fn complete_attempt_orders_every_phase_once() {
    let mut attempt = Esp32s31StaAttempt::with_observer(MockPort::new(None), Observer::default());
    let outcome = block_on(attempt.run(Owner {
        identity: 41,
        completed: 0,
        calls: [None; Esp32s31StaAttemptStage::COUNT as usize],
        call_count: 0,
    }));
    let Esp32s31StaAttemptOutcome::Connected {
        connected,
        progress,
    } = outcome
    else {
        panic!("complete attempt failed");
    };
    assert_eq!(
        connected,
        Connected {
            identity: 41,
            calls: STAGES.map(Some),
        }
    );
    assert_eq!(progress.completed_count(), Esp32s31StaAttemptStage::COUNT);
    for stage in STAGES {
        assert!(progress.completed(stage));
    }
    let (_, observer) = attempt.into_parts();
    assert_eq!(observer.started, STAGES.map(Some));
    assert_eq!(observer.completed, STAGES.map(Some));
    assert_eq!(observer.failed, None);
}

#[test]
fn every_borrowed_phase_failure_returns_the_exact_owner_and_frontier() {
    for (failed_index, failed_stage) in STAGES[..STAGES.len() - 1].iter().enumerate() {
        let mut attempt = Esp32s31StaAttempt::new(MockPort::new(Some(*failed_stage)));
        let outcome = block_on(attempt.run(Owner {
            identity: 77,
            completed: 0,
            calls: [None; Esp32s31StaAttemptStage::COUNT as usize],
            call_count: 0,
        }));
        let Esp32s31StaAttemptOutcome::Failed(failure) = outcome else {
            panic!("failed phase reached connected frontier");
        };
        assert_eq!(failure.owner.identity, 77);
        assert_eq!(failure.owner.completed, failed_index as u8);
        assert_eq!(failure.stage, *failed_stage);
        assert_eq!(
            failure.disposition,
            StaFailureDisposition::RetryCurrentCandidate
        );
        assert_eq!(failure.error, Error(*failed_stage));
        assert_eq!(failure.progress.completed_count(), failed_index as u8);
        for (call, expected) in failure.owner.calls[..=failed_index]
            .iter()
            .zip(&STAGES[..=failed_index])
        {
            assert_eq!(*call, Some(*expected));
        }
    }
}

#[test]
fn connected_entry_failure_must_return_the_consumed_owner() {
    let mut attempt =
        Esp32s31StaAttempt::new(MockPort::new(Some(Esp32s31StaAttemptStage::ConnectedEntry)));
    let outcome = block_on(attempt.run(Owner {
        identity: 91,
        completed: 0,
        calls: [None; Esp32s31StaAttemptStage::COUNT as usize],
        call_count: 0,
    }));
    let Esp32s31StaAttemptOutcome::Failed(failure) = outcome else {
        panic!("connected entry unexpectedly passed");
    };
    assert_eq!(failure.owner.identity, 91);
    assert_eq!(failure.owner.completed, 7);
    assert_eq!(failure.stage, Esp32s31StaAttemptStage::ConnectedEntry);
    assert_eq!(failure.disposition, StaFailureDisposition::Terminal);
    assert_eq!(failure.progress.completed_count(), 7);
    assert!(
        !failure
            .progress
            .completed(Esp32s31StaAttemptStage::ConnectedEntry)
    );
}
