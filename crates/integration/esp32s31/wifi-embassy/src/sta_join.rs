//! Embassy executor for infrastructure-STA Authentication and Association.
//!
//! Protocol policy and state live in `open-esp-radio-ieee80211`. This module
//! owns only executor time and the ordering of finite hardware transactions.
//! A chip backend owns PAC/DMA access and reports each completed descriptor
//! through [`StaJoinRxObserver`]; no vendor context, callback table, NVS,
//! logger, semaphore or allocator is part of this boundary.

use core::future::Future;

use embassy_time::{Instant, Timer};
use open_esp_radio_ieee80211::station::{
    AssociationResponse, StaAssociationAttempt, StaAssociationEvent, StaAssociationFailure,
    StaAssociationRuntime, StaAssociationRuntimeError, StaAuthenticationAttempt,
    StaAuthenticationEvent, StaAuthenticationFailure, StaAuthenticationRuntime,
    StaAuthenticationRuntimeError, StaSequenceCounter,
};

const MICROS_PER_MILLISECOND: u64 = 1_000;

/// Whether a finite RX drain should continue after one completed descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaJoinRxDirective {
    Continue,
    Stop,
}

/// Borrowed management-frame boundary used by a chip-specific RX owner.
///
/// The backend invokes this exactly once for every completed descriptor. A
/// successfully extracted management MPDU is supplied as `Some`; malformed,
/// non-management or otherwise unextractable input is supplied as `None` so
/// protocol diagnostics still retain the complete descriptor count.
pub trait StaJoinRxObserver {
    fn observe_completed(&mut self, management_frame: Option<&[u8]>) -> StaJoinRxDirective;
}

/// Finite PAC/DMA operations required by [`StaJoinRunner`].
///
/// `start_receive` must either publish a live ring or leave no live hardware
/// ownership on error. `service_receive` drains only the currently completed
/// frontier and must honor [`StaJoinRxDirective::Stop`] before recycling the
/// descriptor which produced a terminal protocol event. Association success
/// deliberately leaves RX live so the caller can continue with WPA2 using the
/// same ring epoch.
pub trait StaJoinBackend {
    type Error;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn service_receive<'a, O>(
        &'a mut self,
        observer: &'a mut O,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        O: StaJoinRxObserver + 'a;
}

/// Monotonic executor clock used by the join runner.
pub trait StaJoinTimer {
    fn now_micros(&self) -> u64;
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;
}

/// Production Embassy-time adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyStaJoinTimer;

impl StaJoinTimer for EmbassyStaJoinTimer {
    fn now_micros(&self) -> u64 {
        Instant::now().as_micros()
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        Timer::at(Instant::from_micros(deadline_micros))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAuthenticationSuccess {
    pub attempt: u16,
    pub total_received_frames: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationSuccess {
    pub response: AssociationResponse,
    pub total_received_frames: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaJoinError<E> {
    Backend(E),
    ClockOverflow,
    AuthenticationRuntime(StaAuthenticationRuntimeError),
    AuthenticationFailed {
        attempts: u16,
        failure: StaAuthenticationFailure,
        total_received_frames: u32,
    },
    AssociationRuntime(StaAssociationRuntimeError),
    AssociationFailed {
        failure: StaAssociationFailure,
        total_received_frames: u32,
    },
    InvalidAuthenticationEvent,
    InvalidAssociationEvent,
}

struct AuthenticationObserver<'runtime> {
    runtime: &'runtime mut StaAuthenticationRuntime,
    terminal: Option<Result<StaAuthenticationEvent, StaAuthenticationRuntimeError>>,
}

impl StaJoinRxObserver for AuthenticationObserver<'_> {
    fn observe_completed(&mut self, management_frame: Option<&[u8]>) -> StaJoinRxDirective {
        if let Err(error) = self.runtime.observe_received_frame() {
            self.terminal = Some(Err(error));
            return StaJoinRxDirective::Stop;
        }
        let Some(frame) = management_frame else {
            return StaJoinRxDirective::Continue;
        };
        match self.runtime.observe_management_frame(frame) {
            Ok(StaAuthenticationEvent::Irrelevant) => StaJoinRxDirective::Continue,
            terminal => {
                self.terminal = Some(terminal);
                StaJoinRxDirective::Stop
            }
        }
    }
}

struct AssociationObserver<'runtime> {
    runtime: &'runtime mut StaAssociationRuntime,
    terminal: Option<Result<StaAssociationEvent, StaAssociationRuntimeError>>,
}

impl StaJoinRxObserver for AssociationObserver<'_> {
    fn observe_completed(&mut self, management_frame: Option<&[u8]>) -> StaJoinRxDirective {
        if let Err(error) = self.runtime.observe_received_frame() {
            self.terminal = Some(Err(error));
            return StaJoinRxDirective::Stop;
        }
        let Some(frame) = management_frame else {
            return StaJoinRxDirective::Continue;
        };
        match self.runtime.observe_management_frame(frame) {
            Ok(StaAssociationEvent::Irrelevant) => StaJoinRxDirective::Continue,
            terminal => {
                self.terminal = Some(terminal);
                StaJoinRxDirective::Stop
            }
        }
    }
}

/// Unique Embassy executor for one pre-connected station exchange.
pub struct StaJoinRunner<B, T> {
    backend: B,
    timer: T,
}

impl<B, T> StaJoinRunner<B, T>
where
    B: StaJoinBackend,
    T: StaJoinTimer,
{
    pub const fn new(backend: B, timer: T) -> Self {
        Self { backend, timer }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_parts(self) -> (B, T) {
        (self.backend, self.timer)
    }

    async fn stop_receive(&mut self) -> Result<(), StaJoinError<B::Error>> {
        self.backend
            .stop_receive()
            .await
            .map_err(StaJoinError::Backend)
    }

    async fn wait_boundary(
        &mut self,
        started_micros: u64,
        elapsed_ms: u32,
    ) -> Result<(), StaJoinError<B::Error>> {
        let offset = u64::from(elapsed_ms)
            .checked_mul(MICROS_PER_MILLISECOND)
            .ok_or(StaJoinError::ClockOverflow)?;
        let deadline = started_micros
            .checked_add(offset)
            .ok_or(StaJoinError::ClockOverflow)?;
        self.timer.wait_until_micros(deadline).await;
        Ok(())
    }

    /// Run bounded Open Authentication.
    ///
    /// The exact one-second deadline is measured from completion of each TX
    /// publication. RX is drained at every millisecond boundary, including
    /// the final boundary, before timeout is declared. This makes an RX event
    /// simultaneous with the deadline win deterministically.
    pub async fn authenticate(
        &mut self,
        local: [u8; 6],
        bssid: [u8; 6],
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAuthenticationSuccess, StaJoinError<B::Error>> {
        let mut runtime = StaAuthenticationRuntime::new(local, bssid);
        loop {
            let attempt = runtime
                .begin_attempt(sequence)
                .map_err(StaJoinError::AuthenticationRuntime)?;
            self.backend
                .start_receive()
                .await
                .map_err(StaJoinError::Backend)?;
            if let Err(error) = self.backend.transmit_open_authentication(attempt).await {
                self.stop_receive().await?;
                return Err(StaJoinError::Backend(error));
            }
            let started_micros = self.timer.now_micros();
            let mut terminal = None;
            for elapsed_ms in 1..=attempt.response_timeout_ms {
                self.wait_boundary(started_micros, elapsed_ms).await?;
                let mut observer = AuthenticationObserver {
                    runtime: &mut runtime,
                    terminal: None,
                };
                if let Err(error) = self.backend.service_receive(&mut observer).await {
                    self.stop_receive().await?;
                    return Err(StaJoinError::Backend(error));
                }
                if observer.terminal.is_some() {
                    terminal = observer.terminal;
                    break;
                }
            }
            self.stop_receive().await?;
            let event = match terminal {
                Some(Ok(event)) => event,
                Some(Err(error)) => return Err(StaJoinError::AuthenticationRuntime(error)),
                None => runtime
                    .response_timed_out()
                    .map_err(StaJoinError::AuthenticationRuntime)?,
            };
            match event {
                StaAuthenticationEvent::Authenticated {
                    attempt,
                    total_received_frames,
                } => {
                    return Ok(StaAuthenticationSuccess {
                        attempt,
                        total_received_frames,
                    });
                }
                StaAuthenticationEvent::Retry { .. } => {}
                StaAuthenticationEvent::Failed {
                    attempts,
                    failure,
                    total_received_frames,
                } => {
                    return Err(StaJoinError::AuthenticationFailed {
                        attempts,
                        failure,
                        total_received_frames,
                    });
                }
                StaAuthenticationEvent::Irrelevant => {
                    return Err(StaJoinError::InvalidAuthenticationEvent);
                }
            }
        }
    }

    /// Run one Association epoch and leave RX live only on success.
    ///
    /// Every tick ends at an absolute Embassy deadline, avoiding cumulative
    /// drift from RX parsing or TX publication. The final RX drain occurs at
    /// exactly 1,000 ms before the protocol timeout transition.
    pub async fn associate(
        &mut self,
        local: [u8; 6],
        bssid: [u8; 6],
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAssociationSuccess, StaJoinError<B::Error>> {
        let mut runtime = StaAssociationRuntime::new(local, bssid);
        self.backend
            .start_receive()
            .await
            .map_err(StaJoinError::Backend)?;
        let mut started_micros = None;

        loop {
            let attempt = runtime
                .begin_tick(sequence)
                .map_err(StaJoinError::AssociationRuntime)?;
            if let Some(attempt) = attempt
                && let Err(error) = self.backend.transmit_association(attempt).await
            {
                self.stop_receive().await?;
                return Err(StaJoinError::Backend(error));
            }
            let started_micros = *started_micros.get_or_insert_with(|| self.timer.now_micros());
            let boundary_ms = runtime
                .elapsed_ms()
                .checked_add(1)
                .ok_or(StaJoinError::ClockOverflow)?;
            self.wait_boundary(started_micros, boundary_ms).await?;

            let mut observer = AssociationObserver {
                runtime: &mut runtime,
                terminal: None,
            };
            if let Err(error) = self.backend.service_receive(&mut observer).await {
                self.stop_receive().await?;
                return Err(StaJoinError::Backend(error));
            }
            match observer.terminal {
                Some(Ok(StaAssociationEvent::Associated {
                    response,
                    total_received_frames,
                })) => {
                    return Ok(StaAssociationSuccess {
                        response,
                        total_received_frames,
                    });
                }
                Some(Ok(StaAssociationEvent::Failed {
                    failure,
                    total_received_frames,
                })) => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::AssociationFailed {
                        failure,
                        total_received_frames,
                    });
                }
                Some(Ok(StaAssociationEvent::Irrelevant)) => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::InvalidAssociationEvent);
                }
                Some(Err(error)) => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::AssociationRuntime(error));
                }
                None => {}
            }

            match runtime
                .finish_tick()
                .map_err(StaJoinError::AssociationRuntime)?
            {
                StaAssociationEvent::Irrelevant => {}
                StaAssociationEvent::Failed {
                    failure,
                    total_received_frames,
                } => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::AssociationFailed {
                        failure,
                        total_received_frames,
                    });
                }
                StaAssociationEvent::Associated { .. } => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::InvalidAssociationEvent);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::future::ready;

    use super::*;

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
        const fn new(
            auth_response_poll: Option<u32>,
            association_response_poll: Option<u32>,
        ) -> Self {
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
            embassy_futures::block_on(runner.authenticate(LOCAL, BSSID, &mut sequence)),
            Ok(StaAuthenticationSuccess {
                attempt: 1,
                total_received_frames: 1,
            })
        );
        assert_eq!(
            embassy_futures::block_on(runner.associate(LOCAL, BSSID, &mut sequence)),
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
            embassy_futures::block_on(runner.authenticate(LOCAL, BSSID, &mut sequence)),
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
            embassy_futures::block_on(runner.associate(LOCAL, BSSID, &mut sequence)),
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

        assert!(embassy_futures::block_on(runner.associate(LOCAL, BSSID, &mut sequence)).is_ok());
        assert_eq!(runner.timer.now_micros, 1_000_000);
        assert!(runner.backend().receive_live);
        assert_eq!(runner.backend().stops, 0);
    }
}
