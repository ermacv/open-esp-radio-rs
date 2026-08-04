//! Embassy executor for the WPA2-Personal station four-way handshake.
//!
//! The WPA2 crate owns cryptography, replay state and key-install tickets.
//! This module owns only absolute response deadlines and the ordering of
//! finite RX, RX restart and Message 2 TX transactions. The concrete
//! ESP32-S31 DMA/EAPOL/key-slot/Message-4 binding lives in
//! [`crate::wpa2_port`] rather than in a board or HIL fixture.

use core::future::Future;

use embassy_time::{Instant, Timer};
use open_esp_radio_ieee80211::station::StaSequenceCounter;
use open_esp_radio_wpa2::{
    EapolKeyMessage, OwnedEapolFrame, Pmk, Wpa2Interface,
    aes::AsyncWpa2KeyUnwrap,
    frames::Wpa2TxFrame,
    keys::Wpa2KeyKind,
    supplicant::{
        Wpa2StaKeyInstallRequest, Wpa2StaProcessError, Wpa2StaResponseDeadline,
        Wpa2StaResponseWait, Wpa2StaSupplicant, Wpa2StaSupplicantAction, Wpa2StaSupplicantError,
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

#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyWpa2HandshakeTimer;

impl Wpa2HandshakeTimer for EmbassyWpa2HandshakeTimer {
    fn now_micros(&self) -> u64 {
        Instant::now().as_micros()
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        Timer::at(Instant::from_micros(deadline_micros))
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
    ) -> Result<Wpa2TxFrame<EAPOL_CAPACITY>, Wpa2StaSupplicantError> {
        let Self {
            mut supplicant,
            request,
            ..
        } = self;
        match supplicant.complete_key_install::<EAPOL_CAPACITY>(request, installed)? {
            Wpa2StaSupplicantAction::Transmit(message4) => Ok(message4),
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
    metadata: Wpa2KeyInstallMetadata,
}

impl<Keys> Wpa2Established<Keys> {
    pub const fn metadata(&self) -> Wpa2KeyInstallMetadata {
        self.metadata
    }

    pub fn into_keys(self) -> Keys {
        self.keys
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
        let message4 = match pending.complete(true) {
            Ok(message4) => message4,
            Err(error) => {
                return Err(self.rollback(keys, Wpa2KeyInstallFailure::Complete(error)));
            }
        };
        if message4.key_frame().message() != EapolKeyMessage::PairwiseMessage4
            || message4.key_frame().replay_counter() != replay_counter
            || message4.key_frame().protocol_version() != 1
        {
            return Err(self.rollback(keys, Wpa2KeyInstallFailure::InvalidMessage4));
        }
        metadata.message4_len = message4.as_bytes().len();
        if let Err(error) = self.backend.transmit_message4(&message4, &mut keys).await {
            return Err(self.rollback(keys, Wpa2KeyInstallFailure::Transmit(error)));
        }
        Ok(Wpa2Established { keys, metadata })
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
        sequence: &mut StaSequenceCounter,
    ) -> Result<(), Wpa2HandshakeError<B::Error, U::Error>> {
        if message2.key_frame().message() != open_esp_radio_wpa2::EapolKeyMessage::PairwiseMessage2
        {
            return Err(Wpa2HandshakeError::InvalidMessage2);
        }
        self.backend
            .transmit_message2(message2, sequence.take())
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
        sequence: &mut StaSequenceCounter,
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
                        Ok(Wpa2StaSupplicantAction::None) | Err(_) => {}
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
                open_esp_radio_wpa2::supplicant::Wpa2StaDeadlineEvent::Expired { .. }
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
                open_esp_radio_wpa2::supplicant::Wpa2StaDeadlineEvent::Expired { .. }
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
mod tests {
    use core::future::ready;

    use open_esp_radio_wpa2::{
        PtkContext, Wpa2Interface,
        aes::{AsyncWpa2KeyUnwrap, Wpa2UnwrappedKeyData},
        frames::{OwnedRsnIe, Wpa2Gtk, Wpa2PlainKeyData, Wpa2TxFrame},
        supplicant::{WPA2_STA_MESSAGE1_TIMEOUT_MS, WPA2_STA_MESSAGE3_TIMEOUT_MS},
    };

    use super::*;

    const LOCAL: [u8; 6] = [1; 6];
    const AP: [u8; 6] = [2; 6];
    const SNONCE: [u8; 32] = [3; 32];
    const ANONCE: [u8; 32] = [4; 32];
    const RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestError {
        ReceiveNotLive,
    }

    struct Backend {
        receive_live: bool,
        message1_poll: Option<u32>,
        repeated_message1_poll: Option<u32>,
        message3_poll: Option<u32>,
        message1_polls: u32,
        message3_polls: u32,
        restarts: u16,
        stops: u16,
        transmissions: u16,
        last_sequence: Option<u16>,
    }

    impl Backend {
        const fn new(
            message1_poll: Option<u32>,
            repeated_message1_poll: Option<u32>,
            message3_poll: Option<u32>,
        ) -> Self {
            Self {
                receive_live: true,
                message1_poll,
                repeated_message1_poll,
                message3_poll,
                message1_polls: 0,
                message3_polls: 0,
                restarts: 0,
                stops: 0,
                transmissions: 0,
                last_sequence: None,
            }
        }

        fn message1() -> OwnedEapolFrame<512> {
            let frame = Wpa2TxFrame::<512>::message1(LOCAL, 7, ANONCE).unwrap();
            OwnedEapolFrame::try_copy(Wpa2Interface::Station, AP, frame.as_bytes()).unwrap()
        }

        fn message3() -> OwnedEapolFrame<512> {
            let pmk = Pmk::derive(b"password", b"ssid").unwrap();
            let ptk = pmk.derive_ptk(PtkContext {
                authenticator_address: AP,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            });
            let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
            let gtk = Wpa2Gtk::new(2, false, [0x5a; 16]).unwrap();
            let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
            let frame = Wpa2TxFrame::<512>::message3(
                LOCAL,
                8,
                ANONCE,
                [7, 6, 5, 4, 3, 2, 1, 0],
                plain.as_bytes(),
            )
            .unwrap()
            .authenticate(&ptk);
            OwnedEapolFrame::try_copy(Wpa2Interface::Station, AP, frame.as_bytes()).unwrap()
        }
    }

    impl Wpa2HandshakeBackend for Backend {
        type Error = TestError;

        fn service_receive(
            &mut self,
        ) -> impl Future<Output = Result<Wpa2RxProgress, Self::Error>> + '_ {
            let result = if !self.receive_live {
                Err(TestError::ReceiveNotLive)
            } else if self.restarts == 0 {
                self.message1_polls += 1;
                if self.message1_poll == Some(self.message1_polls) {
                    Ok(Wpa2RxProgress::eapol(1, Self::message1()))
                } else {
                    Ok(Wpa2RxProgress::drained(0))
                }
            } else {
                self.message3_polls += 1;
                if self.message3_poll == Some(self.message3_polls) {
                    Ok(Wpa2RxProgress::eapol(1, Self::message3()))
                } else if self.repeated_message1_poll == Some(self.message3_polls) {
                    Ok(Wpa2RxProgress::eapol(1, Self::message1()))
                } else {
                    Ok(Wpa2RxProgress::drained(0))
                }
            };
            ready(result)
        }

        fn restart_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            self.restarts += 1;
            ready(Ok(()))
        }

        fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            let result = if self.receive_live {
                self.receive_live = false;
                self.stops += 1;
                Ok(())
            } else {
                Err(TestError::ReceiveNotLive)
            };
            ready(result)
        }

        fn transmit_message2(
            &mut self,
            frame: &Wpa2TxFrame<512>,
            sequence_number: u16,
        ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            assert_eq!(
                frame.key_frame().message(),
                open_esp_radio_wpa2::EapolKeyMessage::PairwiseMessage2
            );
            self.transmissions += 1;
            self.last_sequence = Some(sequence_number);
            ready(Ok(()))
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct TestTimer {
        now_micros: u64,
        waits: u32,
    }

    impl Wpa2HandshakeTimer for TestTimer {
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

    fn config<'a>(pmk: &'a Pmk) -> Wpa2HandshakeConfig<'a> {
        Wpa2HandshakeConfig {
            local: LOCAL,
            authenticator: AP,
            supplicant_nonce: SNONCE,
            association_security_ies: &RSN,
            authenticator_rsn_ie: &RSN,
            authenticator_rsnxe: &[],
            pmk,
        }
    }

    struct IdentityUnwrap;

    impl AsyncWpa2KeyUnwrap for IdentityUnwrap {
        type Error = ();

        async fn unwrap_key_data(
            &mut self,
            _kek: &[u8; 16],
            encrypted: &[u8],
        ) -> Result<Wpa2UnwrappedKeyData, Self::Error> {
            Wpa2UnwrappedKeyData::try_copy(encrypted).map_err(|_| ())
        }
    }

    fn pending_key_install() -> Wpa2PendingKeyInstall {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let backend = Backend::new(Some(1), None, Some(1));
        let mut runner = Wpa2HandshakeRunner::new(backend, TestTimer::default(), IdentityUnwrap);
        let mut sequence = StaSequenceCounter::new(0x123);
        embassy_futures::block_on(runner.run(config(&pmk), &mut sequence)).unwrap()
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum KeyBackendError {
        Install,
        Transmit,
    }

    struct KeyBackend {
        fail_install: bool,
        fail_transmit: bool,
        installs: u8,
        transmissions: u8,
        rollbacks: u8,
    }

    impl KeyBackend {
        const fn new(fail_install: bool, fail_transmit: bool) -> Self {
            Self {
                fail_install,
                fail_transmit,
                installs: 0,
                transmissions: 0,
                rollbacks: 0,
            }
        }
    }

    impl Wpa2KeyInstallBackend for KeyBackend {
        type Error = KeyBackendError;
        type InstalledKeys = u8;

        fn install_keys(
            &mut self,
            request: &Wpa2StaKeyInstallRequest,
        ) -> Result<Self::InstalledKeys, Self::Error> {
            assert_eq!(request.replay_counter(), 8);
            assert_eq!(request.pairwise().peer(), &AP);
            if self.fail_install {
                return Err(KeyBackendError::Install);
            }
            self.installs += 1;
            Ok(0xa5)
        }

        fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error> {
            assert_eq!(keys, 0xa5);
            self.rollbacks += 1;
            Ok(())
        }

        fn transmit_message4<'a>(
            &'a mut self,
            frame: &'a Wpa2TxFrame<512>,
            keys: &'a mut Self::InstalledKeys,
        ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
            assert_eq!(*keys, 0xa5);
            assert_eq!(
                frame.key_frame().message(),
                EapolKeyMessage::PairwiseMessage4
            );
            assert_eq!(frame.key_frame().replay_counter(), 8);
            self.transmissions += 1;
            ready(if self.fail_transmit {
                Err(KeyBackendError::Transmit)
            } else {
                Ok(())
            })
        }
    }

    #[test]
    fn key_install_runner_publishes_m4_and_returns_typed_key_ownership() {
        let mut runner = Wpa2KeyInstallRunner::new(KeyBackend::new(false, false));
        let established = embassy_futures::block_on(runner.run(pending_key_install())).unwrap();

        assert_eq!(established.metadata().replay_counter, 8);
        assert_eq!(established.metadata().group_key_id, 2);
        assert_eq!(established.metadata().completed_frames, 2);
        assert_eq!(established.metadata().message2_transmissions, 1);
        assert_eq!(established.into_keys(), 0xa5);
        assert_eq!(runner.backend().installs, 1);
        assert_eq!(runner.backend().transmissions, 1);
        assert_eq!(runner.backend().rollbacks, 0);
    }

    #[test]
    fn message4_tx_failure_rolls_back_both_published_keys() {
        let mut runner = Wpa2KeyInstallRunner::new(KeyBackend::new(false, true));

        assert!(matches!(
            embassy_futures::block_on(runner.run(pending_key_install())),
            Err(Wpa2KeyInstallError::Failed(
                Wpa2KeyInstallFailure::Transmit(KeyBackendError::Transmit)
            ))
        ));
        assert_eq!(runner.backend().installs, 1);
        assert_eq!(runner.backend().transmissions, 1);
        assert_eq!(runner.backend().rollbacks, 1);
    }

    #[test]
    fn atomic_install_failure_does_not_attempt_rollback_or_message4() {
        let mut runner = Wpa2KeyInstallRunner::new(KeyBackend::new(true, false));

        assert!(matches!(
            embassy_futures::block_on(runner.run(pending_key_install())),
            Err(Wpa2KeyInstallError::Install(KeyBackendError::Install))
        ));
        assert_eq!(runner.backend().installs, 0);
        assert_eq!(runner.backend().transmissions, 0);
        assert_eq!(runner.backend().rollbacks, 0);
    }

    #[test]
    fn message1_timeout_is_exact_and_stops_the_live_ring() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let backend = Backend::new(None, None, None);
        let mut runner = Wpa2HandshakeRunner::new(
            backend,
            TestTimer::default(),
            open_esp_radio_wpa2::aes::Wpa2SoftwareAes::new(),
        );
        let mut sequence = StaSequenceCounter::new(0x123);

        assert!(matches!(
            embassy_futures::block_on(runner.run(config(&pmk), &mut sequence)),
            Err(Wpa2HandshakeError::Timeout {
                wait: Wpa2StaResponseWait::Message1,
                elapsed_ms: WPA2_STA_MESSAGE1_TIMEOUT_MS,
                completed_frames: 0,
            })
        ));
        assert_eq!(runner.timer.now_micros, 3_000_000);
        assert_eq!(runner.timer.waits, WPA2_STA_MESSAGE1_TIMEOUT_MS);
        assert!(!runner.backend().receive_live);
        assert_eq!(runner.backend().stops, 1);
    }

    #[test]
    fn peer_message1_sends_m2_once_but_never_retries_it_on_local_timeout() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let backend = Backend::new(Some(1), None, None);
        let mut runner = Wpa2HandshakeRunner::new(
            backend,
            TestTimer::default(),
            open_esp_radio_wpa2::aes::Wpa2SoftwareAes::new(),
        );
        let mut sequence = StaSequenceCounter::new(0x123);

        assert!(matches!(
            embassy_futures::block_on(runner.run(config(&pmk), &mut sequence)),
            Err(Wpa2HandshakeError::Timeout {
                wait: Wpa2StaResponseWait::Message3,
                elapsed_ms: WPA2_STA_MESSAGE3_TIMEOUT_MS,
                completed_frames: 1,
            })
        ));
        assert_eq!(runner.backend().restarts, 1);
        assert_eq!(runner.backend().transmissions, 1);
        assert_eq!(runner.backend().last_sequence, Some(0x123));
        assert_eq!(sequence.peek(), 0x124);
        assert_eq!(runner.timer.now_micros, 6_001_000);
        assert_eq!(
            runner.timer.waits,
            WPA2_STA_MESSAGE1_TIMEOUT_MS.min(1) + WPA2_STA_MESSAGE3_TIMEOUT_MS
        );
    }

    #[test]
    fn repeated_peer_message1_is_the_only_message2_refresh_source() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let backend = Backend::new(Some(1), Some(100), None);
        let mut runner = Wpa2HandshakeRunner::new(
            backend,
            TestTimer::default(),
            open_esp_radio_wpa2::aes::Wpa2SoftwareAes::new(),
        );
        let mut sequence = StaSequenceCounter::new(7);

        assert!(matches!(
            embassy_futures::block_on(runner.run(config(&pmk), &mut sequence)),
            Err(Wpa2HandshakeError::Timeout {
                wait: Wpa2StaResponseWait::Message3,
                ..
            })
        ));
        assert_eq!(runner.backend().transmissions, 2);
        assert_eq!(sequence.peek(), 9);
        assert_eq!(runner.timer.now_micros, 6_001_000);
    }

    #[test]
    fn message1_on_exact_deadline_is_serviced_before_timeout() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let backend = Backend::new(Some(WPA2_STA_MESSAGE1_TIMEOUT_MS), None, None);
        let mut runner = Wpa2HandshakeRunner::new(
            backend,
            TestTimer::default(),
            open_esp_radio_wpa2::aes::Wpa2SoftwareAes::new(),
        );
        let mut sequence = StaSequenceCounter::new(0);

        assert!(matches!(
            embassy_futures::block_on(runner.run(config(&pmk), &mut sequence)),
            Err(Wpa2HandshakeError::Timeout {
                wait: Wpa2StaResponseWait::Message3,
                ..
            })
        ));
        assert_eq!(runner.backend().transmissions, 1);
        assert_eq!(runner.timer.now_micros, 9_000_000);
    }
}
