//! Hardware-free application boundary for runtime radio supervision.
//!
//! Physical owners remain in one local supervisor actor. This module contains
//! only value reports and a controller over an implementation-provided port;
//! it deliberately does not model a detached owner-holding service.

use core::future::Future;

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

/// Wi-Fi view exposed by [`RadioController`].
pub struct WifiController<P> {
    port: P,
}

impl<P> WifiController<P> {
    pub const fn new(port: P) -> Self {
        Self { port }
    }

    pub fn into_port(self) -> P {
        self.port
    }
}

impl<P: WifiSupervisorPort> WifiController<P> {
    pub async fn start_station(
        &mut self,
        request: StationRequest,
    ) -> Result<WifiStartReport, WifiStartFailure<StationRequest, P::Error>> {
        self.port.start_station(request).await
    }

    pub async fn start_monitor(
        &mut self,
        request: MonitorRequest,
    ) -> Result<WifiStartReport, WifiStartFailure<MonitorRequest, P::Error>> {
        self.port.start_monitor(request).await
    }

    /// Completion means the owner-holding actor confirmed quiescence. Merely
    /// publishing stop intent is never returned as success.
    pub async fn stop(&mut self) -> Result<WifiStopReport, P::Error> {
        self.port.stop().await
    }
}

/// Application radio handle containing control capability only.
///
/// Physical RF, protocol owners, PAC, DMA and interrupt epochs remain in the
/// supervisor actor and cannot be extracted through this value.
pub struct RadioController<W> {
    wifi: WifiController<W>,
}

impl<W> RadioController<W> {
    pub const fn new(wifi: WifiController<W>) -> Self {
        Self { wifi }
    }

    pub const fn wifi(&mut self) -> &mut WifiController<W> {
        &mut self.wifi
    }

    pub fn into_wifi(self) -> WifiController<W> {
        self.wifi
    }
}
