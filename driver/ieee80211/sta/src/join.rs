//! Executor-independent infrastructure-STA Authentication and Association.
//!
//! Wire parsing and response values live in `open-esp-radio-ieee80211`. This
//! module owns Authentication/Association state, retry policy and the ordering
//! and absolute deadlines of finite hardware transactions, receiving time
//! through an explicit port.
//! A chip backend owns PAC/DMA access and reports each completed descriptor
//! through [`StaJoinRxObserver`]; no vendor context, callback table, NVS,
//! logger, semaphore or allocator is part of this boundary.

use core::future::Future;

use open_esp_radio_ieee80211::security::WifiSecurityMode;
use open_esp_radio_ieee80211::station::{AssociationResponse, StaSequenceCounter};

use self::association::{
    StaAssociationAttempt, StaAssociationEvent, StaAssociationFailure, StaAssociationRuntime,
    StaAssociationRuntimeError,
};
use self::authentication::{
    StaAuthenticationAttempt, StaAuthenticationEvent, StaAuthenticationFailure,
    StaAuthenticationRuntime, StaAuthenticationRuntimeError,
};

pub mod association;
pub mod authentication;

#[cfg(test)]
mod test_support;

/// Vendor state timer used by ordinary Authentication and Association.
///
/// SOURCE: complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state`, ordinary non-mesh auth branch `.L347` and
/// association branch `.L353`, both arm their software timer with immediate
/// `0x3e8`.
pub const STA_RESPONSE_TIMEOUT_MS: u32 = 1_000;

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

/// Monotonic clock used by the join runner.
pub trait StaJoinTimer {
    fn now_micros(&self) -> u64;
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;
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

/// Unique transaction runner for one pre-connected station exchange.
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
    /// Every tick ends at an absolute deadline, avoiding cumulative
    /// drift from RX parsing or TX publication. The final RX drain occurs at
    /// exactly 1,000 ms before the protocol timeout transition.
    pub async fn associate(
        &mut self,
        local: [u8; 6],
        bssid: [u8; 6],
        security: WifiSecurityMode,
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAssociationSuccess, StaJoinError<B::Error>> {
        let mut runtime = StaAssociationRuntime::new(local, bssid, security);
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
mod tests;
