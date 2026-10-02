//! Executor-independent infrastructure-STA Authentication and Association.
//!
//! Wire parsing and response values live in `oer-ieee80211-mac`. This
//! module owns Authentication/Association state and retry policy as state
//! machines, and declares [`StaJoinBackend`], the port of finite hardware
//! transactions that the `StaJoinRunner` of `oer-ieee80211-sta-service`
//! orders against absolute deadlines.
//! A chip backend owns PAC/DMA access and reports each completed descriptor
//! through [`StaJoinRxObserver`]; no vendor context, callback table, NVS,
//! logger, semaphore or allocator is part of this boundary.

use core::future::Future;

use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::station::AssociationResponse;

use self::association::{StaAssociationAttempt, StaAssociationFailure, StaAssociationRuntimeError};
use self::authentication::{
    StaAuthenticationAttempt, StaAuthenticationFailure, StaAuthenticationRuntimeError,
};
use self::sae::{StaSaeFailure, StaSaeTransmission};

pub mod association;
pub mod authentication;
pub mod sae;

#[cfg(test)]
mod test_support;

/// Vendor state timer used by ordinary Authentication and Association.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state`, ordinary non-mesh auth branch `.L350` and
/// association branch `.L356`, both arm their software timer with immediate
/// `0x3e8`.
pub const STA_RESPONSE_TIMEOUT_MS: u32 = 1_000;

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

/// Finite PAC/DMA operations required by the `StaJoinRunner` of
/// `oer-ieee80211-sta-service`.
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

    /// Publish one SAE Authentication frame with its own management
    /// sequence number.
    fn transmit_sae_authentication<'a>(
        &'a mut self,
        sequence_number: SequenceNumber,
        transmission: &'a StaSaeTransmission,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;

    fn service_receive<'a, O>(
        &'a mut self,
        observer: &'a mut O,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        O: StaJoinRxObserver + 'a;
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
    SaeFailed(StaSaeFailure),
}
