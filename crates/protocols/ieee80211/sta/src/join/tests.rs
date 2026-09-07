use core::future::ready;

use super::*;
use crate::test_support::block_on;

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Authentication,
    Association,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestError {
    ReceiveAlreadyStarted,
    ReceiveNotStarted,
}

struct Backend {
    phase: Phase,
    receive_live: bool,
    auth_response_poll: Option<u32>,
    association_response_poll: Option<u32>,
    auth_polls: u32,
    association_polls: u32,
    auth_attempts: [Option<StaAuthenticationAttempt>; 3],
    auth_attempt_count: usize,
    association_attempts: [Option<StaAssociationAttempt>; 7],
    association_attempt_count: usize,
    starts: u16,
    stops: u16,
}

impl Backend {
    const fn new(auth_response_poll: Option<u32>, association_response_poll: Option<u32>) -> Self {
        Self {
            phase: Phase::Idle,
            receive_live: false,
            auth_response_poll,
            association_response_poll,
            auth_polls: 0,
            association_polls: 0,
            auth_attempts: [None; 3],
            auth_attempt_count: 0,
            association_attempts: [None; 7],
            association_attempt_count: 0,
            starts: 0,
            stops: 0,
        }
    }
}

impl StaJoinBackend for Backend {
    type Error = TestError;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        let result = if self.receive_live {
            Err(TestError::ReceiveAlreadyStarted)
        } else {
            self.receive_live = true;
            self.starts += 1;
            Ok(())
        };
        ready(result)
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        let result = if self.receive_live {
            self.receive_live = false;
            self.phase = Phase::Idle;
            self.stops += 1;
            Ok(())
        } else {
            Err(TestError::ReceiveNotStarted)
        };
        ready(result)
    }

    fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        self.phase = Phase::Authentication;
        self.auth_attempts[self.auth_attempt_count] = Some(attempt);
        self.auth_attempt_count += 1;
        ready(Ok(()))
    }

    fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        self.phase = Phase::Association;
        self.association_attempts[self.association_attempt_count] = Some(attempt);
        self.association_attempt_count += 1;
        ready(Ok(()))
    }

    fn service_receive<'a, O>(
        &'a mut self,
        observer: &'a mut O,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        O: StaJoinRxObserver + 'a,
    {
        let result = if !self.receive_live {
            Err(TestError::ReceiveNotStarted)
        } else {
            match self.phase {
                Phase::Authentication => {
                    self.auth_polls += 1;
                    if self.auth_response_poll == Some(self.auth_polls) {
                        let _ = observer.observe_completed(Some(&authentication_response(0)));
                    }
                }
                Phase::Association => {
                    self.association_polls += 1;
                    if self.association_response_poll == Some(self.association_polls) {
                        let _ = observer.observe_completed(Some(&association_response(0)));
                    }
                }
                Phase::Idle => {}
            }
            Ok(())
        };
        ready(result)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct TestTimer {
    now_micros: u64,
    waits: u32,
}

impl StaJoinTimer for TestTimer {
    fn now_micros(&self) -> u64 {
        self.now_micros
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        assert!(deadline_micros >= self.now_micros);
        self.now_micros = deadline_micros;
        self.waits += 1;
        ready(())
    }
}

fn authentication_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    frame[28..30].copy_from_slice(&status_code.to_le_bytes());
    frame
}

fn association_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x0010_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
    frame[26..28].copy_from_slice(&status_code.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}

#[test]
fn successful_join_uses_typed_sequences_and_leaves_association_rx_live() {
    let backend = Backend::new(Some(1), Some(2));
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(0x123);

    assert_eq!(
        block_on(runner.authenticate(LOCAL, BSSID, &mut sequence)),
        Ok(StaAuthenticationSuccess {
            attempt: 1,
            total_received_frames: 1,
        })
    );
    assert_eq!(
        block_on(runner.associate(LOCAL, BSSID, WifiSecurityMode::Wpa2Personal, &mut sequence,)),
        Ok(StaAssociationSuccess {
            response: AssociationResponse {
                capability_info: 0x0431,
                status_code: 0,
                association_id: 42,
                ht_capability: false,
                he_capability: false,
                he_operation: false,
                wmm: false,
                wmm_parameters: None,
            },
            total_received_frames: 1,
        })
    );

    assert_eq!(
        runner.backend().auth_attempts[0].unwrap().sequence_number,
        0x123
    );
    assert_eq!(
        runner.backend().association_attempts[0]
            .unwrap()
            .sequence_number,
        0x124
    );
    assert!(runner.backend().receive_live);
    assert_eq!(runner.backend().starts, 2);
    assert_eq!(runner.backend().stops, 1);
}

#[test]
fn authentication_timeout_is_three_exact_one_second_epochs() {
    let backend = Backend::new(None, None);
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(0);

    assert_eq!(
        block_on(runner.authenticate(LOCAL, BSSID, &mut sequence)),
        Err(StaJoinError::AuthenticationFailed {
            attempts: 3,
            failure: StaAuthenticationFailure::Timeout,
            total_received_frames: 0,
        })
    );
    assert_eq!(runner.backend().auth_attempt_count, 3);
    assert_eq!(runner.backend().starts, 3);
    assert_eq!(runner.backend().stops, 3);
    assert!(!runner.backend().receive_live);
    assert_eq!(runner.timer.now_micros, 3_000_000);
    assert_eq!(runner.timer.waits, 3_000);
}

#[test]
fn association_timeout_sends_seven_requests_and_stops_rx_at_1000_ms() {
    let backend = Backend::new(None, None);
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(7);

    assert_eq!(
        block_on(runner.associate(LOCAL, BSSID, WifiSecurityMode::Wpa2Personal, &mut sequence,)),
        Err(StaJoinError::AssociationFailed {
            failure: StaAssociationFailure::Timeout,
            total_received_frames: 0,
        })
    );
    assert_eq!(runner.backend().association_attempt_count, 7);
    assert_eq!(
        runner
            .backend()
            .association_attempts
            .map(|attempt| attempt.unwrap().elapsed_ms),
        [0, 160, 320, 480, 640, 800, 960]
    );
    assert_eq!(runner.timer.now_micros, 1_000_000);
    assert_eq!(runner.timer.waits, 1_000);
    assert!(!runner.backend().receive_live);
    assert_eq!(runner.backend().starts, 1);
    assert_eq!(runner.backend().stops, 1);
}

#[test]
fn association_response_on_exact_deadline_wins_before_timeout() {
    let backend = Backend::new(None, Some(1_000));
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(0);

    assert!(
        block_on(runner.associate(LOCAL, BSSID, WifiSecurityMode::Wpa2Personal, &mut sequence,))
            .is_ok()
    );
    assert_eq!(runner.timer.now_micros, 1_000_000);
    assert!(runner.backend().receive_live);
    assert_eq!(runner.backend().stops, 0);
}
