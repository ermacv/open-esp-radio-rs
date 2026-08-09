//! Hardware-free application boundary for runtime radio supervision.
//!
//! Physical owners remain in one local supervisor actor. This module contains
//! only value reports and a controller over an implementation-provided port;
//! it deliberately does not model a detached owner-holding service.

use core::{fmt, future::Future};

use crate::{MonitorRequest, StationRequest};

/// Wrapping identity of one successfully started Wi-Fi service epoch.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RadioSubsystemGeneration(u32);

impl RadioSubsystemGeneration {
    pub const INITIAL: Self = Self(0);

    pub const fn value(self) -> u32 {
        self.0
    }

    /// Allocate the identity of the next successfully materialized epoch.
    ///
    /// Wrapping is intentional: this is a stale-completion discriminator, not
    /// a globally unique persistent identifier.
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

pub type WifiStartResult<R, E> = Result<WifiStartReport, WifiStartFailure<R, E>>;

/// Successful role start acknowledged by the owner-holding supervisor actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiStartReport {
    generation: RadioSubsystemGeneration,
}

impl WifiStartReport {
    pub const fn new(generation: RadioSubsystemGeneration) -> Self {
        Self { generation }
    }

    pub const fn generation(self) -> RadioSubsystemGeneration {
        self.generation
    }
}

/// Successful stop after the active role returned its DMA/ISR owner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiStopReport {
    generation: RadioSubsystemGeneration,
}

impl WifiStopReport {
    pub const fn new(generation: RadioSubsystemGeneration) -> Self {
        Self { generation }
    }

    pub const fn generation(self) -> RadioSubsystemGeneration {
        self.generation
    }
}

/// Start failure at the application control boundary.
///
/// A command rejected before hardware moves returns the untouched request. A
/// faulted result cannot return it because materialization may already retain
/// request-owned security state at a quarantined hardware frontier.
#[derive(Debug)]
pub enum WifiStartFailure<R, E> {
    Rejected { request: R, error: E },
    Faulted { error: E },
}

impl<R, E> WifiStartFailure<R, E> {
    pub const fn rejected(request: R, error: E) -> Self {
        Self::Rejected { request, error }
    }

    pub const fn faulted(error: E) -> Self {
        Self::Faulted { error }
    }
}

/// Hardware-free transport implemented by the physical supervisor mailbox.
///
/// Implementations publish requests to the single local actor which owns the
/// radio. They never execute ownership transitions in the application task.
pub trait WifiSupervisorPort {
    type Error;

    fn start_station(
        &mut self,
        request: StationRequest,
    ) -> impl Future<Output = WifiStartResult<StationRequest, Self::Error>> + '_;

    fn start_monitor(
        &mut self,
        request: MonitorRequest,
    ) -> impl Future<Output = WifiStartResult<MonitorRequest, Self::Error>> + '_;

    fn stop(&mut self) -> impl Future<Output = Result<WifiStopReport, Self::Error>> + '_;
}

/// Role-neutral Wi-Fi control capability.
///
/// This is the only state from which a role can be started. Starting a role
/// consumes it, so station and monitor ownership cannot overlap in safe code.
pub struct WifiIdle<P> {
    port: P,
}

impl<P> WifiIdle<P> {
    pub const fn new(port: P) -> Self {
        Self { port }
    }

    pub fn into_port(self) -> P {
        self.port
    }
}

impl<P: WifiSupervisorPort> WifiIdle<P> {
    pub async fn start_station(
        mut self,
        request: StationRequest,
    ) -> Result<WifiStation<P>, WifiRoleStartFailure<Self, StationRequest, P::Error>> {
        match self.port.start_station(request).await {
            Ok(report) => Ok(WifiStation {
                port: self.port,
                generation: report.generation(),
            }),
            Err(WifiStartFailure::Rejected { request, error }) => {
                Err(WifiRoleStartFailure::Rejected {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiStartFailure::Faulted { error }) => {
                Err(WifiRoleStartFailure::Faulted { error })
            }
        }
    }

    pub async fn start_monitor(
        mut self,
        request: MonitorRequest,
    ) -> Result<WifiMonitor<P>, WifiRoleStartFailure<Self, MonitorRequest, P::Error>> {
        match self.port.start_monitor(request).await {
            Ok(report) => Ok(WifiMonitor {
                port: self.port,
                generation: report.generation(),
            }),
            Err(WifiStartFailure::Rejected { request, error }) => {
                Err(WifiRoleStartFailure::Rejected {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiStartFailure::Faulted { error }) => {
                Err(WifiRoleStartFailure::Faulted { error })
            }
        }
    }
}

/// Failed role start. A request rejected before hardware moved returns both
/// the request and the idle capability; a terminal hardware fault returns
/// neither because it is not legal to start another role.
pub enum WifiRoleStartFailure<W, R, E> {
    Rejected { wifi: W, request: R, error: E },
    Faulted { error: E },
}

impl<W, R: fmt::Debug, E: fmt::Debug> fmt::Debug for WifiRoleStartFailure<W, R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { request, error, .. } => formatter
                .debug_struct("Rejected")
                .field("request", request)
                .field("error", error)
                .finish(),
            Self::Faulted { error } => formatter
                .debug_struct("Faulted")
                .field("error", error)
                .finish(),
        }
    }
}

/// Active station role. It is neither `Clone` nor `Copy`; stopping consumes it
/// and returns [`WifiIdle`] only after the runner confirms quiescence.
pub struct WifiStation<P> {
    port: P,
    generation: RadioSubsystemGeneration,
}

impl<P: WifiSupervisorPort> WifiStation<P> {
    pub const fn generation(&self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub async fn stop(mut self) -> Result<WifiIdle<P>, WifiRoleStopFailure<P::Error>> {
        match self.port.stop().await {
            Ok(report) if report.generation() == self.generation => Ok(WifiIdle::new(self.port)),
            Ok(_) => Err(WifiRoleStopFailure::GenerationMismatch),
            Err(error) => Err(WifiRoleStopFailure::Faulted(error)),
        }
    }
}

/// Active standalone monitor role.
pub struct WifiMonitor<P> {
    port: P,
    generation: RadioSubsystemGeneration,
}

impl<P: WifiSupervisorPort> WifiMonitor<P> {
    pub const fn generation(&self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub async fn stop(mut self) -> Result<WifiIdle<P>, WifiRoleStopFailure<P::Error>> {
        match self.port.stop().await {
            Ok(report) if report.generation() == self.generation => Ok(WifiIdle::new(self.port)),
            Ok(_) => Err(WifiRoleStopFailure::GenerationMismatch),
            Err(error) => Err(WifiRoleStopFailure::Faulted(error)),
        }
    }
}

/// A role can return to idle only through a matching quiesced completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRoleStopFailure<E> {
    GenerationMismatch,
    Faulted(E),
}

/// Application radio handle containing control capability only.
///
/// Physical RF, protocol owners, PAC, DMA and interrupt epochs remain in the
/// supervisor actor and cannot be extracted through this value.
pub struct RadioController<W> {
    wifi: WifiIdle<W>,
}

impl<W> RadioController<W> {
    pub const fn new(wifi: WifiIdle<W>) -> Self {
        Self { wifi }
    }

    /// Materialize the sole Wi-Fi control capability. The radio controller is
    /// consumed so a second Wi-Fi root cannot be produced.
    pub fn into_wifi(self) -> WifiIdle<W> {
        self.wifi
    }
}
