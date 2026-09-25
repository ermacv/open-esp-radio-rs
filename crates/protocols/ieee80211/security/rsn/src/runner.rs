//! Executor-independent WPA2-Personal station four-way-handshake runner.
//!
//! This module owns absolute response deadlines, key-publication rollback and
//! the ordering of finite RX, RX restart and EAPOL TX transactions. Concrete
//! DMA, timer, key-slot and frame-transmit bindings remain in chip/runtime
//! adapters.

use core::future::Future;

use crate::{
    EapolKeyMessage, OwnedEapolFrame, Pmk, RsnInterface,
    aes::AsyncRsnKeyUnwrap,
    frames::RsnTxFrame,
    keys::RsnKeyKind,
    supplicant::{
        RsnConnectedSupplicant, RsnStaKeyInstallRequest, RsnStaProcessError,
        RsnStaResponseDeadline, RsnStaResponseWait, RsnStaSupplicant, RsnStaSupplicantAction,
        RsnStaSupplicantError,
    },
};

const EAPOL_CAPACITY: usize = 512;
const MICROS_PER_MILLISECOND: u64 = 1_000;

/// One finite RX pass. `more` requests another pass at the same executor
/// boundary, normally because the backend stopped after copying one EAPOL
/// frame and deliberately left later completed descriptors untouched.
pub struct RsnRxProgress {
    pub completed_frames: u32,
    pub eapol: Option<OwnedEapolFrame<EAPOL_CAPACITY>>,
    pub more: bool,
}

impl RsnRxProgress {
    pub const fn drained(completed_frames: u32) -> Self {
        Self {
            completed_frames,
            eapol: None,
            more: false,
        }
    }

    pub const fn eapol(completed_frames: u32, eapol: OwnedEapolFrame<EAPOL_CAPACITY>) -> Self {
        Self {
            completed_frames,
            eapol: Some(eapol),
            more: true,
        }
    }
}

/// Finite hardware operations required by [`RsnHandshakeRunner`].
///
/// The backend enters with the Association RX ring live. It copies at most
/// one EAPOL packet into Rust-owned storage per pass, releases every PAC/DMA
/// borrow before returning, and leaves RX stopped on successful return from
/// the runner so key installation has an unambiguous hardware boundary.
pub trait RsnHandshakeBackend {
    type Error;

    fn service_receive(&mut self) -> impl Future<Output = Result<RsnRxProgress, Self::Error>> + '_;

    fn restart_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn transmit_message2<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<EAPOL_CAPACITY>,
        sequence_number: u16,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

pub trait RsnHandshakeTimer {
    fn now_micros(&self) -> u64;
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;
}

/// HMAC-owned sequence-number source used for EAPOL Message 2.
///
/// WPA2 consumes a number but does not own the IEEE 802.11 sequence space.
/// The callback-shaped blanket implementation keeps this crate independent
/// from a particular HMAC state type.
pub trait RsnTxSequence {
    fn take_sequence(&mut self) -> u16;
}

impl<F: FnMut() -> u16> RsnTxSequence for F {
    fn take_sequence(&mut self) -> u16 {
        self()
    }
}

pub struct RsnHandshakeConfig<'config> {
    pub local: [u8; 6],
    pub authenticator: [u8; 6],
    pub supplicant_nonce: [u8; 32],
    pub association_security_ies: &'config [u8],
    pub authenticator_rsn_ie: &'config [u8],
    pub authenticator_rsnxe: &'config [u8],
    pub pmk: &'config Pmk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnHandshakeError<BackendError, UnwrapError> {
    Backend(BackendError),
    ClockOverflow,
    Create(RsnStaSupplicantError),
    Process(RsnStaProcessError<UnwrapError>),
    InvalidMessage2,
    UnexpectedAction,
    Timeout {
        wait: RsnStaResponseWait,
        elapsed_ms: u32,
        completed_frames: u32,
    },
}

/// Protocol state and exact key-install ticket returned with RX stopped.
pub struct RsnPendingKeyInstall {
    supplicant: RsnStaSupplicant,
    request: RsnStaKeyInstallRequest,
    completed_frames: u32,
    message2_transmissions: u16,
}

pub struct RsnCompletedKeyInstall {
    pub message4: RsnTxFrame<EAPOL_CAPACITY>,
    pub connected: RsnConnectedSupplicant,
}

impl RsnPendingKeyInstall {
    pub const fn request(&self) -> &RsnStaKeyInstallRequest {
        &self.request
    }

    pub const fn completed_frames(&self) -> u32 {
        self.completed_frames
    }

    pub const fn message2_transmissions(&self) -> u16 {
        self.message2_transmissions
    }

    pub fn complete(
        self,
        installed: bool,
    ) -> Result<RsnCompletedKeyInstall, RsnStaSupplicantError> {
        let Self {
            mut supplicant,
            request,
            ..
        } = self;
        match supplicant.complete_key_install::<EAPOL_CAPACITY>(request, installed)? {
            RsnStaSupplicantAction::Transmit(message4) => Ok(RsnCompletedKeyInstall {
                message4,
                connected: supplicant.into_connected()?,
            }),
            _ => Err(RsnStaSupplicantError::UnexpectedAction),
        }
    }
}

/// Finite hardware boundary for publishing a validated PTK/GTK pair and
/// transmitting the corresponding Message 4.
///
/// `install_keys` must be atomic from the caller's point of view: on error it
/// leaves no published key behind. Once it succeeds, every later error is
/// routed through `rollback_keys` before the runner returns.
pub trait RsnKeyInstallBackend {
    type Error;
    type InstalledKeys;

    fn install_keys(
        &mut self,
        request: &RsnStaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error>;

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error>;

    fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<EAPOL_CAPACITY>,
        keys: &'a mut Self::InstalledKeys,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnKeyInstallRequestError {
    PairwiseInterface,
    PairwiseKind,
    GroupInterface,
    GroupKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnKeyInstallMetadata {
    pub replay_counter: u64,
    pub encrypted_key_data: bool,
    pub plain_key_data_len: usize,
    pub group_key_id: u8,
    pub group_transmit: bool,
    pub completed_frames: u32,
    pub message2_transmissions: u16,
    pub message4_len: usize,
}

pub struct RsnEstablished<Keys> {
    keys: Keys,
    connected: RsnConnectedSupplicant,
    metadata: RsnKeyInstallMetadata,
}

impl<Keys> RsnEstablished<Keys> {
    pub const fn metadata(&self) -> RsnKeyInstallMetadata {
        self.metadata
    }

    pub fn into_parts(self) -> (Keys, RsnConnectedSupplicant) {
        (self.keys, self.connected)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum RsnKeyInstallFailure<BackendError> {
    Complete(RsnStaSupplicantError),
    InvalidMessage4,
    Transmit(BackendError),
}

#[derive(Debug, Eq, PartialEq)]
pub enum RsnKeyInstallError<BackendError> {
    Request(RsnKeyInstallRequestError),
    Install(BackendError),
    Failed(RsnKeyInstallFailure<BackendError>),
    Rollback {
        failure: RsnKeyInstallFailure<BackendError>,
        rollback: BackendError,
    },
}

/// Owns the security-critical ordering between the supplicant ticket and the
/// chip-specific key/TX backend. It contains no credentials, diagnostics,
/// retry timer or connected-network policy.
pub struct RsnKeyInstallRunner<B> {
    backend: B,
}

impl<B: RsnKeyInstallBackend> RsnKeyInstallRunner<B> {
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    fn rollback(
        &mut self,
        keys: B::InstalledKeys,
        failure: RsnKeyInstallFailure<B::Error>,
    ) -> RsnKeyInstallError<B::Error> {
        match self.backend.rollback_keys(keys) {
            Ok(()) => RsnKeyInstallError::Failed(failure),
            Err(rollback) => RsnKeyInstallError::Rollback { failure, rollback },
        }
    }

    pub async fn run(
        &mut self,
        pending: RsnPendingKeyInstall,
    ) -> Result<RsnEstablished<B::InstalledKeys>, RsnKeyInstallError<B::Error>> {
        let request = pending.request();
        let pairwise = request.pairwise();
        let group = request.group();
        let request_error = if pairwise.interface() != RsnInterface::Station {
            Some(RsnKeyInstallRequestError::PairwiseInterface)
        } else if pairwise.kind() != RsnKeyKind::Pairwise {
            Some(RsnKeyInstallRequestError::PairwiseKind)
        } else if group.interface() != RsnInterface::Station {
            Some(RsnKeyInstallRequestError::GroupInterface)
        } else if !matches!(group.kind(), RsnKeyKind::Group { .. }) {
            Some(RsnKeyInstallRequestError::GroupKind)
        } else {
            None
        };
        if let Some(error) = request_error {
            let _ = pending.complete(false);
            return Err(RsnKeyInstallError::Request(error));
        }
        let RsnKeyKind::Group {
            key_id: group_key_id,
            transmit: group_transmit,
        } = group.kind()
        else {
            unreachable!("group kind was validated above")
        };
        let replay_counter = request.replay_counter();
        let mut metadata = RsnKeyInstallMetadata {
            replay_counter,
            encrypted_key_data: request.encrypted_key_data(),
            plain_key_data_len: request.plain_key_data_len(),
            group_key_id,
            group_transmit,
            completed_frames: pending.completed_frames(),
            message2_transmissions: pending.message2_transmissions(),
            message4_len: 0,
        };
        let mut keys = match self.backend.install_keys(request) {
            Ok(keys) => keys,
            Err(error) => {
                let _ = pending.complete(false);
                return Err(RsnKeyInstallError::Install(error));
            }
        };
        let completed = match pending.complete(true) {
            Ok(completed) => completed,
            Err(error) => {
                return Err(self.rollback(keys, RsnKeyInstallFailure::Complete(error)));
            }
        };
        if completed.message4.key_frame().message() != EapolKeyMessage::PairwiseMessage4
            || completed.message4.key_frame().replay_counter() != replay_counter
            || completed.message4.key_frame().protocol_version() != 1
        {
            return Err(self.rollback(keys, RsnKeyInstallFailure::InvalidMessage4));
        }
        metadata.message4_len = completed.message4.as_bytes().len();
        if let Err(error) = self
            .backend
            .transmit_message4(&completed.message4, &mut keys)
            .await
        {
            return Err(self.rollback(keys, RsnKeyInstallFailure::Transmit(error)));
        }
        Ok(RsnEstablished {
            keys,
            connected: completed.connected,
            metadata,
        })
    }
}

pub struct RsnHandshakeRunner<B, T, U> {
    backend: B,
    timer: T,
    key_unwrap: U,
}

impl<B, T, U> RsnHandshakeRunner<B, T, U>
where
    B: RsnHandshakeBackend,
    T: RsnHandshakeTimer,
    U: AsyncRsnKeyUnwrap,
{
    pub const fn new(backend: B, timer: T, key_unwrap: U) -> Self {
        Self {
            backend,
            timer,
            key_unwrap,
        }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn into_parts(self) -> (B, T, U) {
        (self.backend, self.timer, self.key_unwrap)
    }

    async fn stop_receive(&mut self) -> Result<(), RsnHandshakeError<B::Error, U::Error>> {
        self.backend
            .stop_receive()
            .await
            .map_err(RsnHandshakeError::Backend)
    }

    async fn wait_boundary(
        &mut self,
        started_micros: u64,
        elapsed_ms: u32,
    ) -> Result<(), RsnHandshakeError<B::Error, U::Error>> {
        let offset = u64::from(elapsed_ms)
            .checked_mul(MICROS_PER_MILLISECOND)
            .ok_or(RsnHandshakeError::ClockOverflow)?;
        let deadline = started_micros
            .checked_add(offset)
            .ok_or(RsnHandshakeError::ClockOverflow)?;
        self.timer.wait_until_micros(deadline).await;
        Ok(())
    }

    async fn transmit_message2(
        &mut self,
        message2: &RsnTxFrame<EAPOL_CAPACITY>,
        sequence: &mut impl RsnTxSequence,
    ) -> Result<(), RsnHandshakeError<B::Error, U::Error>> {
        if message2.key_frame().message() != crate::EapolKeyMessage::PairwiseMessage2 {
            return Err(RsnHandshakeError::InvalidMessage2);
        }
        self.backend
            .transmit_message2(message2, sequence.take_sequence())
            .await
            .map_err(RsnHandshakeError::Backend)
    }

    /// Drive Message 1 and Message 3, returning the exact typed key ticket.
    ///
    /// RX is serviced at the absolute deadline before timeout wins. Message 2
    /// is never retransmitted from a local timer; only a repeated peer Message
    /// 1 can produce another Message 2, preserving the recovered vendor
    /// behavior.
    pub async fn run(
        &mut self,
        config: RsnHandshakeConfig<'_>,
        sequence: &mut impl RsnTxSequence,
    ) -> Result<RsnPendingKeyInstall, RsnHandshakeError<B::Error, U::Error>> {
        let mut supplicant = RsnStaSupplicant::try_new(
            config.local,
            config.authenticator,
            config.supplicant_nonce,
            config.association_security_ies,
            config.authenticator_rsn_ie,
            config.authenticator_rsnxe,
        )
        .map_err(RsnHandshakeError::Create)?;
        let mut completed_frames = 0_u32;
        let mut message2_transmissions = 0_u16;
        let mut message1_deadline = RsnStaResponseDeadline::new(RsnStaResponseWait::Message1);
        let message1_started = self.timer.now_micros();

        'message1: loop {
            let boundary = message1_deadline
                .elapsed_ms()
                .checked_add(1)
                .ok_or(RsnHandshakeError::ClockOverflow)?;
            self.wait_boundary(message1_started, boundary).await?;
            loop {
                let progress = match self.backend.service_receive().await {
                    Ok(progress) => progress,
                    Err(error) => {
                        self.stop_receive().await?;
                        return Err(RsnHandshakeError::Backend(error));
                    }
                };
                completed_frames = completed_frames.saturating_add(progress.completed_frames);
                if let Some(frame) = progress.eapol {
                    let replay_counter = frame.key_frame().replay_counter();
                    match supplicant
                        .on_frame(frame, config.pmk, &mut self.key_unwrap)
                        .await
                    {
                        Ok(RsnStaSupplicantAction::Transmit(message2))
                            if message2.key_frame().replay_counter() == replay_counter =>
                        {
                            if let Err(error) = self.backend.restart_receive().await {
                                self.stop_receive().await?;
                                return Err(RsnHandshakeError::Backend(error));
                            }
                            if let Err(error) = self.transmit_message2(&message2, sequence).await {
                                self.stop_receive().await?;
                                return Err(error);
                            }
                            message2_transmissions = message2_transmissions.saturating_add(1);
                            break 'message1;
                        }
                        Ok(RsnStaSupplicantAction::None) => {}
                        Err(error) if error.is_peer_input_rejection() => {}
                        Err(error) => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::Process(error));
                        }
                        Ok(_) => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::UnexpectedAction);
                        }
                    }
                }
                if !progress.more {
                    break;
                }
            }
            if matches!(
                message1_deadline.finish_millisecond(),
                crate::supplicant::RsnStaDeadlineEvent::Expired { .. }
            ) {
                self.stop_receive().await?;
                return Err(RsnHandshakeError::Timeout {
                    wait: RsnStaResponseWait::Message1,
                    elapsed_ms: message1_deadline.elapsed_ms(),
                    completed_frames,
                });
            }
        }

        let mut message3_deadline = RsnStaResponseDeadline::new(RsnStaResponseWait::Message3);
        let message3_started = self.timer.now_micros();
        loop {
            let boundary = message3_deadline
                .elapsed_ms()
                .checked_add(1)
                .ok_or(RsnHandshakeError::ClockOverflow)?;
            self.wait_boundary(message3_started, boundary).await?;
            loop {
                let progress = match self.backend.service_receive().await {
                    Ok(progress) => progress,
                    Err(error) => {
                        self.stop_receive().await?;
                        return Err(RsnHandshakeError::Backend(error));
                    }
                };
                completed_frames = completed_frames.saturating_add(progress.completed_frames);
                if let Some(frame) = progress.eapol {
                    let action = match supplicant
                        .on_frame(frame, config.pmk, &mut self.key_unwrap)
                        .await
                    {
                        Ok(action) => action,
                        Err(error) if error.is_peer_input_rejection() => {
                            RsnStaSupplicantAction::None
                        }
                        Err(error) => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::Process(error));
                        }
                    };
                    match action {
                        RsnStaSupplicantAction::Transmit(message2) => {
                            if let Err(error) = self.transmit_message2(&message2, sequence).await {
                                self.stop_receive().await?;
                                return Err(error);
                            }
                            message2_transmissions = message2_transmissions.saturating_add(1);
                        }
                        RsnStaSupplicantAction::InstallKeys(request) => {
                            self.stop_receive().await?;
                            return Ok(RsnPendingKeyInstall {
                                supplicant,
                                request,
                                completed_frames,
                                message2_transmissions,
                            });
                        }
                        RsnStaSupplicantAction::None => {}
                        RsnStaSupplicantAction::Deauthenticate => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::UnexpectedAction);
                        }
                    }
                }
                if !progress.more {
                    break;
                }
            }
            if matches!(
                message3_deadline.finish_millisecond(),
                crate::supplicant::RsnStaDeadlineEvent::Expired { .. }
            ) {
                self.stop_receive().await?;
                return Err(RsnHandshakeError::Timeout {
                    wait: RsnStaResponseWait::Message3,
                    elapsed_ms: message3_deadline.elapsed_ms(),
                    completed_frames,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
