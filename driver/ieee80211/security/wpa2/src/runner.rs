//! Executor-independent WPA2-Personal station four-way-handshake runner.
//!
//! This module owns absolute response deadlines, key-publication rollback and
//! the ordering of finite RX, RX restart and EAPOL TX transactions. Concrete
//! DMA, timer, key-slot and frame-transmit bindings remain in chip/runtime
//! adapters.

use core::future::Future;

use crate::{
    EapolKeyMessage, OwnedEapolFrame, Pmk, Wpa2Interface,
    aes::AsyncWpa2KeyUnwrap,
    frames::Wpa2TxFrame,
    keys::Wpa2KeyKind,
    supplicant::{
        Wpa2ConnectedSupplicant, Wpa2StaKeyInstallRequest, Wpa2StaProcessError,
        Wpa2StaResponseDeadline, Wpa2StaResponseWait, Wpa2StaSupplicant, Wpa2StaSupplicantAction,
        Wpa2StaSupplicantError,
    },
};

const EAPOL_CAPACITY: usize = 512;
const MICROS_PER_MILLISECOND: u64 = 1_000;

/// One finite RX pass. `more` requests another pass at the same executor
/// boundary, normally because the backend stopped after copying one EAPOL
/// frame and deliberately left later completed descriptors untouched.
pub struct Wpa2RxProgress {
    pub completed_frames: u32,
    pub eapol: Option<OwnedEapolFrame<EAPOL_CAPACITY>>,
    pub more: bool,
}

impl Wpa2RxProgress {
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

/// Finite hardware operations required by [`Wpa2HandshakeRunner`].
///
/// The backend enters with the Association RX ring live. It copies at most
/// one EAPOL packet into Rust-owned storage per pass, releases every PAC/DMA
/// borrow before returning, and leaves RX stopped on successful return from
/// the runner so key installation has an unambiguous hardware boundary.
pub trait Wpa2HandshakeBackend {
    type Error;

    fn service_receive(&mut self)
    -> impl Future<Output = Result<Wpa2RxProgress, Self::Error>> + '_;

    fn restart_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn transmit_message2<'a>(
        &'a mut self,
        frame: &'a Wpa2TxFrame<EAPOL_CAPACITY>,
        sequence_number: u16,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

pub trait Wpa2HandshakeTimer {
    fn now_micros(&self) -> u64;
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;
}

/// HMAC-owned sequence-number source used for EAPOL Message 2.
///
/// WPA2 consumes a number but does not own the IEEE 802.11 sequence space.
/// The callback-shaped blanket implementation keeps this crate independent
/// from a particular HMAC state type.
pub trait Wpa2TxSequence {
    fn take_sequence(&mut self) -> u16;
}

impl<F: FnMut() -> u16> Wpa2TxSequence for F {
    fn take_sequence(&mut self) -> u16 {
        self()
    }
}

pub struct Wpa2HandshakeConfig<'config> {
    pub local: [u8; 6],
    pub authenticator: [u8; 6],
    pub supplicant_nonce: [u8; 32],
    pub association_security_ies: &'config [u8],
    pub authenticator_rsn_ie: &'config [u8],
    pub authenticator_rsnxe: &'config [u8],
    pub pmk: &'config Pmk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2HandshakeError<BackendError, UnwrapError> {
    Backend(BackendError),
    ClockOverflow,
    Create(Wpa2StaSupplicantError),
    Process(Wpa2StaProcessError<UnwrapError>),
    InvalidMessage2,
    UnexpectedAction,
    Timeout {
        wait: Wpa2StaResponseWait,
        elapsed_ms: u32,
        completed_frames: u32,
    },
}

/// Protocol state and exact key-install ticket returned with RX stopped.
pub struct Wpa2PendingKeyInstall {
    supplicant: Wpa2StaSupplicant,
    request: Wpa2StaKeyInstallRequest,
    completed_frames: u32,
    message2_transmissions: u16,
}

pub struct Wpa2CompletedKeyInstall {
    pub message4: Wpa2TxFrame<EAPOL_CAPACITY>,
    pub connected: Wpa2ConnectedSupplicant,
}

impl Wpa2PendingKeyInstall {
    pub const fn request(&self) -> &Wpa2StaKeyInstallRequest {
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
    ) -> Result<Wpa2CompletedKeyInstall, Wpa2StaSupplicantError> {
        let Self {
            mut supplicant,
            request,
            ..
        } = self;
        match supplicant.complete_key_install::<EAPOL_CAPACITY>(request, installed)? {
            Wpa2StaSupplicantAction::Transmit(message4) => Ok(Wpa2CompletedKeyInstall {
                message4,
                connected: supplicant.into_connected()?,
            }),
            _ => Err(Wpa2StaSupplicantError::UnexpectedAction),
        }
    }
}

/// Finite hardware boundary for publishing a validated PTK/GTK pair and
/// transmitting the corresponding Message 4.
///
/// `install_keys` must be atomic from the caller's point of view: on error it
/// leaves no published key behind. Once it succeeds, every later error is
/// routed through `rollback_keys` before the runner returns.
pub trait Wpa2KeyInstallBackend {
    type Error;
    type InstalledKeys;

    fn install_keys(
        &mut self,
        request: &Wpa2StaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error>;

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error>;

    fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a Wpa2TxFrame<EAPOL_CAPACITY>,
        keys: &'a mut Self::InstalledKeys,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2KeyInstallRequestError {
    PairwiseInterface,
    PairwiseKind,
    GroupInterface,
    GroupKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2KeyInstallMetadata {
    pub replay_counter: u64,
    pub encrypted_key_data: bool,
    pub plain_key_data_len: usize,
    pub group_key_id: u8,
    pub group_transmit: bool,
    pub completed_frames: u32,
    pub message2_transmissions: u16,
    pub message4_len: usize,
}

pub struct Wpa2Established<Keys> {
    keys: Keys,
    connected: Wpa2ConnectedSupplicant,
    metadata: Wpa2KeyInstallMetadata,
}

impl<Keys> Wpa2Established<Keys> {
    pub const fn metadata(&self) -> Wpa2KeyInstallMetadata {
        self.metadata
    }

    pub fn into_parts(self) -> (Keys, Wpa2ConnectedSupplicant) {
        (self.keys, self.connected)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2KeyInstallFailure<BackendError> {
    Complete(Wpa2StaSupplicantError),
    InvalidMessage4,
    Transmit(BackendError),
}

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2KeyInstallError<BackendError> {
    Request(Wpa2KeyInstallRequestError),
    Install(BackendError),
    Failed(Wpa2KeyInstallFailure<BackendError>),
    Rollback {
        failure: Wpa2KeyInstallFailure<BackendError>,
        rollback: BackendError,
    },
}

/// Owns the security-critical ordering between the supplicant ticket and the
/// chip-specific key/TX backend. It contains no credentials, diagnostics,
/// retry timer or connected-network policy.
pub struct Wpa2KeyInstallRunner<B> {
    backend: B,
}

impl<B: Wpa2KeyInstallBackend> Wpa2KeyInstallRunner<B> {
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
        failure: Wpa2KeyInstallFailure<B::Error>,
    ) -> Wpa2KeyInstallError<B::Error> {
        match self.backend.rollback_keys(keys) {
            Ok(()) => Wpa2KeyInstallError::Failed(failure),
            Err(rollback) => Wpa2KeyInstallError::Rollback { failure, rollback },
        }
    }

    pub async fn run(
        &mut self,
        pending: Wpa2PendingKeyInstall,
    ) -> Result<Wpa2Established<B::InstalledKeys>, Wpa2KeyInstallError<B::Error>> {
        let request = pending.request();
        let pairwise = request.pairwise();
        let group = request.group();
        let request_error = if pairwise.interface() != Wpa2Interface::Station {
            Some(Wpa2KeyInstallRequestError::PairwiseInterface)
        } else if pairwise.kind() != Wpa2KeyKind::Pairwise {
            Some(Wpa2KeyInstallRequestError::PairwiseKind)
        } else if group.interface() != Wpa2Interface::Station {
            Some(Wpa2KeyInstallRequestError::GroupInterface)
        } else if !matches!(group.kind(), Wpa2KeyKind::Group { .. }) {
            Some(Wpa2KeyInstallRequestError::GroupKind)
        } else {
            None
        };
        if let Some(error) = request_error {
            let _ = pending.complete(false);
            return Err(Wpa2KeyInstallError::Request(error));
        }
        let Wpa2KeyKind::Group {
            key_id: group_key_id,
            transmit: group_transmit,
        } = group.kind()
        else {
            unreachable!("group kind was validated above")
        };
        let replay_counter = request.replay_counter();
        let mut metadata = Wpa2KeyInstallMetadata {
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
                return Err(Wpa2KeyInstallError::Install(error));
            }
        };
        let completed = match pending.complete(true) {
            Ok(completed) => completed,
            Err(error) => {
                return Err(self.rollback(keys, Wpa2KeyInstallFailure::Complete(error)));
            }
        };
        if completed.message4.key_frame().message() != EapolKeyMessage::PairwiseMessage4
            || completed.message4.key_frame().replay_counter() != replay_counter
            || completed.message4.key_frame().protocol_version() != 1
        {
            return Err(self.rollback(keys, Wpa2KeyInstallFailure::InvalidMessage4));
        }
        metadata.message4_len = completed.message4.as_bytes().len();
        if let Err(error) = self
            .backend
            .transmit_message4(&completed.message4, &mut keys)
            .await
        {
            return Err(self.rollback(keys, Wpa2KeyInstallFailure::Transmit(error)));
        }
        Ok(Wpa2Established {
            keys,
            connected: completed.connected,
            metadata,
        })
    }
}

pub struct Wpa2HandshakeRunner<B, T, U> {
    backend: B,
    timer: T,
    key_unwrap: U,
}

impl<B, T, U> Wpa2HandshakeRunner<B, T, U>
where
    B: Wpa2HandshakeBackend,
    T: Wpa2HandshakeTimer,
    U: AsyncWpa2KeyUnwrap,
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

    async fn stop_receive(&mut self) -> Result<(), Wpa2HandshakeError<B::Error, U::Error>> {
        self.backend
            .stop_receive()
            .await
            .map_err(Wpa2HandshakeError::Backend)
    }

    async fn wait_boundary(
        &mut self,
        started_micros: u64,
        elapsed_ms: u32,
    ) -> Result<(), Wpa2HandshakeError<B::Error, U::Error>> {
        let offset = u64::from(elapsed_ms)
            .checked_mul(MICROS_PER_MILLISECOND)
            .ok_or(Wpa2HandshakeError::ClockOverflow)?;
        let deadline = started_micros
            .checked_add(offset)
            .ok_or(Wpa2HandshakeError::ClockOverflow)?;
        self.timer.wait_until_micros(deadline).await;
        Ok(())
    }

    async fn transmit_message2(
        &mut self,
        message2: &Wpa2TxFrame<EAPOL_CAPACITY>,
        sequence: &mut impl Wpa2TxSequence,
    ) -> Result<(), Wpa2HandshakeError<B::Error, U::Error>> {
        if message2.key_frame().message() != crate::EapolKeyMessage::PairwiseMessage2 {
            return Err(Wpa2HandshakeError::InvalidMessage2);
        }
        self.backend
            .transmit_message2(message2, sequence.take_sequence())
            .await
            .map_err(Wpa2HandshakeError::Backend)
    }

    /// Drive Message 1 and Message 3, returning the exact typed key ticket.
    ///
    /// RX is serviced at the absolute deadline before timeout wins. Message 2
    /// is never retransmitted from a local timer; only a repeated peer Message
    /// 1 can produce another Message 2, preserving the recovered vendor
    /// behavior.
    pub async fn run(
        &mut self,
        config: Wpa2HandshakeConfig<'_>,
        sequence: &mut impl Wpa2TxSequence,
    ) -> Result<Wpa2PendingKeyInstall, Wpa2HandshakeError<B::Error, U::Error>> {
        let mut supplicant = Wpa2StaSupplicant::try_new(
            config.local,
            config.authenticator,
            config.supplicant_nonce,
            config.association_security_ies,
            config.authenticator_rsn_ie,
            config.authenticator_rsnxe,
        )
        .map_err(Wpa2HandshakeError::Create)?;
        let mut completed_frames = 0_u32;
        let mut message2_transmissions = 0_u16;
        let mut message1_deadline = Wpa2StaResponseDeadline::new(Wpa2StaResponseWait::Message1);
        let message1_started = self.timer.now_micros();

        'message1: loop {
            let boundary = message1_deadline
                .elapsed_ms()
                .checked_add(1)
                .ok_or(Wpa2HandshakeError::ClockOverflow)?;
            self.wait_boundary(message1_started, boundary).await?;
            loop {
                let progress = match self.backend.service_receive().await {
                    Ok(progress) => progress,
                    Err(error) => {
                        self.stop_receive().await?;
                        return Err(Wpa2HandshakeError::Backend(error));
                    }
                };
                completed_frames = completed_frames.saturating_add(progress.completed_frames);
                if let Some(frame) = progress.eapol {
                    let replay_counter = frame.key_frame().replay_counter();
                    match supplicant
                        .on_frame(frame, config.pmk, &mut self.key_unwrap)
                        .await
                    {
                        Ok(Wpa2StaSupplicantAction::Transmit(message2))
                            if message2.key_frame().replay_counter() == replay_counter =>
                        {
                            if let Err(error) = self.backend.restart_receive().await {
                                self.stop_receive().await?;
                                return Err(Wpa2HandshakeError::Backend(error));
                            }
                            if let Err(error) = self.transmit_message2(&message2, sequence).await {
                                self.stop_receive().await?;
                                return Err(error);
                            }
                            message2_transmissions = message2_transmissions.saturating_add(1);
                            break 'message1;
                        }
                        Ok(Wpa2StaSupplicantAction::None) => {}
                        Err(error) if error.is_peer_input_rejection() => {}
                        Err(error) => {
                            self.stop_receive().await?;
                            return Err(Wpa2HandshakeError::Process(error));
                        }
                        Ok(_) => {
                            self.stop_receive().await?;
                            return Err(Wpa2HandshakeError::UnexpectedAction);
                        }
                    }
                }
                if !progress.more {
                    break;
                }
            }
            if matches!(
                message1_deadline.finish_millisecond(),
                crate::supplicant::Wpa2StaDeadlineEvent::Expired { .. }
            ) {
                self.stop_receive().await?;
                return Err(Wpa2HandshakeError::Timeout {
                    wait: Wpa2StaResponseWait::Message1,
                    elapsed_ms: message1_deadline.elapsed_ms(),
                    completed_frames,
                });
            }
        }

        let mut message3_deadline = Wpa2StaResponseDeadline::new(Wpa2StaResponseWait::Message3);
        let message3_started = self.timer.now_micros();
        loop {
            let boundary = message3_deadline
                .elapsed_ms()
                .checked_add(1)
                .ok_or(Wpa2HandshakeError::ClockOverflow)?;
            self.wait_boundary(message3_started, boundary).await?;
            loop {
                let progress = match self.backend.service_receive().await {
                    Ok(progress) => progress,
                    Err(error) => {
                        self.stop_receive().await?;
                        return Err(Wpa2HandshakeError::Backend(error));
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
                            Wpa2StaSupplicantAction::None
                        }
                        Err(error) => {
                            self.stop_receive().await?;
                            return Err(Wpa2HandshakeError::Process(error));
                        }
                    };
                    match action {
                        Wpa2StaSupplicantAction::Transmit(message2) => {
                            if let Err(error) = self.transmit_message2(&message2, sequence).await {
                                self.stop_receive().await?;
                                return Err(error);
                            }
                            message2_transmissions = message2_transmissions.saturating_add(1);
                        }
                        Wpa2StaSupplicantAction::InstallKeys(request) => {
                            self.stop_receive().await?;
                            return Ok(Wpa2PendingKeyInstall {
                                supplicant,
                                request,
                                completed_frames,
                                message2_transmissions,
                            });
                        }
                        Wpa2StaSupplicantAction::None => {}
                        Wpa2StaSupplicantAction::Deauthenticate => {
                            self.stop_receive().await?;
                            return Err(Wpa2HandshakeError::UnexpectedAction);
                        }
                    }
                }
                if !progress.more {
                    break;
                }
            }
            if matches!(
                message3_deadline.finish_millisecond(),
                crate::supplicant::Wpa2StaDeadlineEvent::Expired { .. }
            ) {
                self.stop_receive().await?;
                return Err(Wpa2HandshakeError::Timeout {
                    wait: Wpa2StaResponseWait::Message3,
                    elapsed_ms: message3_deadline.elapsed_ms(),
                    completed_frames,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
