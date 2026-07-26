//! Allocation-free async WPA2 AP four-way-handshake transitions.
//!
//! Each function consumes owned ingress and produces owned radio commands.
//! The caller remains the orchestration owner and submits commands to the
//! bounded FIFO in the order returned here.

use crate::{
    wpa2::{OwnedEapolFrame, Wpa2Interface},
    wpa2_aes::AsyncWpa2KeyWrap,
    wpa2_crypto::{new_mic_job, Wpa2Ptk},
    wpa2_frames::{
        build_ap_action_frame, OwnedRsnIe, Wpa2EthernetFrame, Wpa2FrameError, Wpa2Gtk,
        Wpa2PlainKeyData, WPA2_PLAIN_KEY_DATA_CAPACITY, WPA2_TX_EAPOL_CAPACITY,
        WPA2_TX_ETHERNET_CAPACITY,
    },
    wpa2_io::{Wpa2IoCommand, Wpa2KeyInstall},
    wpa2_sta_async::AsyncWpa2StaCrypto,
    wpa2_state::{Wpa2ApAction, Wpa2ApState, Wpa2StateError, Wpa2Ticket},
    CryptoJobError,
};

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2ApStartError {
    State(Wpa2StateError),
    Frame(Wpa2FrameError),
    UnexpectedAction,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2ApMessage2Error<E> {
    State(Wpa2StateError),
    MicJob(CryptoJobError),
    Crypto(E),
    Message2MicMismatch,
    UnexpectedAction,
}

/// PTK and M3-preparation ticket produced after a valid message 2.
///
/// The temporal key is deliberately not installed into hardware here. M3 is
/// an unprotected 802.11 frame whose GTK key data is protected inside EAPOL;
/// installing the PTK before M3 would make the generic AP TX path encrypt a
/// frame that the supplicant cannot yet receive.
pub struct Wpa2ApMessage2 {
    ticket: Wpa2Ticket,
    ptk: Wpa2Ptk,
}

impl Wpa2ApMessage2 {
    pub fn into_parts(self) -> (Wpa2Ticket, Wpa2Ptk) {
        (self.ticket, self.ptk)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2ApMessage3Error<S, A> {
    State(Wpa2StateError),
    Frame(Wpa2FrameError),
    Sha1(S),
    KeyWrap(A),
    Message3PreparationFailed,
    UnexpectedAction,
}

/// Persistent PTK plus the owned M3 frame queued after its pairwise key.
pub struct Wpa2ApMessage3 {
    ptk: Wpa2Ptk,
    frame: Wpa2EthernetFrame<WPA2_TX_ETHERNET_CAPACITY>,
}

/// Pairwise-key installation and controlled-port commands released only
/// after M4 has proved that the supplicant accepted M3.
pub struct Wpa2ApMessage4 {
    pairwise_key: Wpa2KeyInstall,
    authorize: Wpa2IoCommand,
}

impl Wpa2ApMessage4 {
    pub fn into_commands(self) -> (Wpa2IoCommand, Wpa2IoCommand) {
        (Wpa2IoCommand::InstallKey(self.pairwise_key), self.authorize)
    }
}

impl Wpa2ApMessage3 {
    pub const fn ptk(&self) -> &Wpa2Ptk {
        &self.ptk
    }

    pub const fn frame(&self) -> &Wpa2EthernetFrame<WPA2_TX_ETHERNET_CAPACITY> {
        &self.frame
    }

    pub fn into_parts(self) -> (Wpa2Ptk, Wpa2EthernetFrame<WPA2_TX_ETHERNET_CAPACITY>) {
        (self.ptk, self.frame)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2ApMessage4Error<E> {
    State(Wpa2StateError),
    MicJob(CryptoJobError),
    Crypto(E),
    Message4MicMismatch,
    UnexpectedAction,
}

/// Build the first owned AP EAPOL frame without retaining a peer-table borrow.
pub fn start_wpa2_ap_handshake(
    state: &Wpa2ApState,
) -> Result<Wpa2EthernetFrame<WPA2_TX_ETHERNET_CAPACITY>, Wpa2ApStartError> {
    let transmit = match state.message1(false).map_err(Wpa2ApStartError::State)? {
        Wpa2ApAction::Transmit(transmit) => transmit,
        _ => return Err(Wpa2ApStartError::UnexpectedAction),
    };
    let eapol = build_ap_action_frame::<WPA2_TX_EAPOL_CAPACITY>(state, transmit, [0; 8], &[])
        .map_err(Wpa2ApStartError::Frame)?;
    Wpa2EthernetFrame::build(*state.local_address(), &eapol).map_err(Wpa2ApStartError::Frame)
}

/// Derive the PTK, verify M2, and produce the AP pairwise-key command.
pub async fn complete_wpa2_ap_message2<C, const N: usize>(
    state: &mut Wpa2ApState,
    message2: OwnedEapolFrame<N>,
    pmk: &[u8; 32],
    crypto: &mut C,
) -> Result<Wpa2ApMessage2, Wpa2ApMessage2Error<C::Error>>
where
    C: AsyncWpa2StaCrypto,
{
    let (ptk_ticket, context, retained) = match state
        .on_frame(message2)
        .map_err(Wpa2ApMessage2Error::State)?
    {
        Wpa2ApAction::DerivePtk {
            ticket,
            context,
            message2,
        } => (ticket, context, message2),
        _ => return Err(Wpa2ApMessage2Error::UnexpectedAction),
    };

    let ptk = match crypto.derive_ptk(pmk, context).await {
        Ok(ptk) => Wpa2Ptk::from_bytes(ptk),
        Err(error) => {
            let _ = state.complete_ptk(ptk_ticket, retained, false);
            return Err(Wpa2ApMessage2Error::Crypto(error));
        }
    };
    let (mic_ticket, retained) = match state
        .complete_ptk(ptk_ticket, retained, true)
        .map_err(Wpa2ApMessage2Error::State)?
    {
        Wpa2ApAction::VerifyMessage2Mic { ticket, message2 } => (ticket, message2),
        _ => return Err(Wpa2ApMessage2Error::UnexpectedAction),
    };
    let expected_mic = *retained.key_frame().mic();
    let mic_job = new_mic_job(ptk.kck(), &retained).map_err(Wpa2ApMessage2Error::MicJob)?;
    let actual_mic = crypto
        .hmac_sha1(ptk.kck(), mic_job.input())
        .await
        .map_err(Wpa2ApMessage2Error::Crypto)?;
    let valid_mic = constant_time_equal(&actual_mic[..16], &expected_mic);
    let install_ticket = match state
        .complete_message2_mic(mic_ticket, retained, valid_mic)
        .map_err(Wpa2ApMessage2Error::State)?
    {
        Wpa2ApAction::PrepareMessage3 { ticket } if valid_mic => ticket,
        Wpa2ApAction::DeauthenticatePeer if !valid_mic => {
            return Err(Wpa2ApMessage2Error::Message2MicMismatch)
        }
        _ => return Err(Wpa2ApMessage2Error::UnexpectedAction),
    };
    Ok(Wpa2ApMessage2 {
        ticket: install_ticket,
        ptk,
    })
}

/// Build MIC-protected EAPOL M3 while leaving its 802.11 carrier plaintext.
pub async fn complete_wpa2_ap_message3<S, A, const R: usize>(
    state: &mut Wpa2ApState,
    ticket: Wpa2Ticket,
    ptk: Wpa2Ptk,
    rsn_ie: &OwnedRsnIe<R>,
    gtk: &Wpa2Gtk,
    key_rsc: [u8; 8],
    sha1: &mut S,
    aes: &mut A,
) -> Result<Wpa2ApMessage3, Wpa2ApMessage3Error<S::Error, A::Error>>
where
    S: AsyncWpa2StaCrypto,
    A: AsyncWpa2KeyWrap,
{
    let transmit = match state
        .complete_message3_preparation::<WPA2_TX_EAPOL_CAPACITY>(ticket, true)
        .map_err(Wpa2ApMessage3Error::State)?
    {
        Wpa2ApAction::Transmit(transmit) => transmit,
        Wpa2ApAction::DeauthenticatePeer => {
            return Err(Wpa2ApMessage3Error::Message3PreparationFailed)
        }
        _ => return Err(Wpa2ApMessage3Error::UnexpectedAction),
    };
    let plain = Wpa2PlainKeyData::<WPA2_PLAIN_KEY_DATA_CAPACITY>::build(rsn_ie, gtk)
        .map_err(Wpa2ApMessage3Error::Frame)?;
    let wrapped = aes
        .wrap_key_data(ptk.kek(), plain.as_bytes())
        .await
        .map_err(Wpa2ApMessage3Error::KeyWrap)?;
    let mut eapol = build_ap_action_frame::<WPA2_TX_EAPOL_CAPACITY>(
        state,
        transmit,
        key_rsc,
        wrapped.as_bytes(),
    )
    .map_err(Wpa2ApMessage3Error::Frame)?;
    let mic = sha1
        .hmac_sha1(ptk.kck(), eapol.as_bytes())
        .await
        .map_err(Wpa2ApMessage3Error::Sha1)?;
    eapol.set_mic(
        mic[..16]
            .try_into()
            .expect("a SHA-1 tag always contains a 16-byte WPA2 MIC"),
    );
    let frame = Wpa2EthernetFrame::build(*state.local_address(), &eapol)
        .map_err(Wpa2ApMessage3Error::Frame)?;
    Ok(Wpa2ApMessage3 { ptk, frame })
}

/// Verify M4 and produce the controlled-port command for the radio owner.
pub async fn complete_wpa2_ap_message4<C, const N: usize>(
    state: &mut Wpa2ApState,
    message4: OwnedEapolFrame<N>,
    ptk: &Wpa2Ptk,
    crypto: &mut C,
) -> Result<Wpa2ApMessage4, Wpa2ApMessage4Error<C::Error>>
where
    C: AsyncWpa2StaCrypto,
{
    let (ticket, retained) = match state
        .on_frame(message4)
        .map_err(Wpa2ApMessage4Error::State)?
    {
        Wpa2ApAction::VerifyMessage4Mic { ticket, message4 } => (ticket, message4),
        _ => return Err(Wpa2ApMessage4Error::UnexpectedAction),
    };
    let expected_mic = *retained.key_frame().mic();
    let mic_job = new_mic_job(ptk.kck(), &retained).map_err(Wpa2ApMessage4Error::MicJob)?;
    let actual_mic = crypto
        .hmac_sha1(ptk.kck(), mic_job.input())
        .await
        .map_err(Wpa2ApMessage4Error::Crypto)?;
    let valid_mic = constant_time_equal(&actual_mic[..16], &expected_mic);
    match state
        .complete_message4_mic(ticket, retained, valid_mic)
        .map_err(Wpa2ApMessage4Error::State)?
    {
        Wpa2ApAction::AuthorizePeer if valid_mic => Ok(Wpa2ApMessage4 {
            pairwise_key: Wpa2KeyInstall::pairwise(
                Wpa2Interface::AccessPoint,
                *state.peer(),
                [0; 8],
                ptk,
            ),
            authorize: Wpa2IoCommand::SetPeerAuthorized {
                interface: Wpa2Interface::AccessPoint,
                peer: *state.peer(),
                authorized: true,
            },
        }),
        Wpa2ApAction::DeauthenticatePeer if !valid_mic => {
            Err(Wpa2ApMessage4Error::Message4MicMismatch)
        }
        _ => Err(Wpa2ApMessage4Error::UnexpectedAction),
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use super::*;
    use crate::{
        wpa2::{EapolKeyFrame, EapolKeyMessage},
        wpa2_aes::Wpa2SoftwareAes,
        wpa2_frames::Wpa2TxFrame,
        wpa2_state::Wpa2ApPhase,
    };

    struct Crypto;

    impl AsyncWpa2StaCrypto for Crypto {
        type Error = ();

        async fn derive_ptk(
            &mut self,
            _pmk: &[u8; 32],
            _context: crate::PtkContext,
        ) -> Result<[u8; 48], Self::Error> {
            Ok([0x55; 48])
        }

        async fn hmac_sha1(
            &mut self,
            _key: &[u8; 16],
            message: &[u8],
        ) -> Result<[u8; 20], Self::Error> {
            assert_eq!(&message[81..97], &[0; 16]);
            Ok([0xa5; 20])
        }
    }

    #[test]
    fn owned_ap_flow_orders_key_m3_and_authorization() {
        let ap = [1; 6];
        let peer = [2; 6];
        let mut state = Wpa2ApState::new(ap, peer, [3; 32], 7).unwrap();
        let m1 = start_wpa2_ap_handshake(&state).unwrap();
        assert_eq!(
            EapolKeyFrame::parse(&m1.as_bytes()[14..])
                .unwrap()
                .message(),
            EapolKeyMessage::PairwiseMessage1
        );

        let mut m2 = Wpa2TxFrame::<128>::message2(peer, 7, [4; 32], &rsn()).unwrap();
        m2.set_mic(&[0xa5; 16]);
        let m2: OwnedEapolFrame<128> =
            OwnedEapolFrame::try_copy(Wpa2Interface::AccessPoint, peer, m2.as_bytes()).unwrap();
        let mut crypto = Crypto;
        let completion = run_ready(complete_wpa2_ap_message2(
            &mut state,
            m2,
            &[9; 32],
            &mut crypto,
        ))
        .unwrap();
        let (ticket, ptk) = completion.into_parts();

        let gtk = Wpa2Gtk::new(1, true, [0x77; 16]).unwrap();
        let mut aes = Wpa2SoftwareAes::new();
        let m3 = run_ready(complete_wpa2_ap_message3(
            &mut state,
            ticket,
            ptk,
            &rsn(),
            &gtk,
            [0; 8],
            &mut crypto,
            &mut aes,
        ))
        .unwrap();
        assert_eq!(
            EapolKeyFrame::parse(&m3.frame().as_bytes()[14..])
                .unwrap()
                .message(),
            EapolKeyMessage::PairwiseMessage3
        );

        let mut m4 = Wpa2TxFrame::<128>::message4(peer, 8).unwrap();
        m4.set_mic(&[0xa5; 16]);
        let m4: OwnedEapolFrame<128> =
            OwnedEapolFrame::try_copy(Wpa2Interface::AccessPoint, peer, m4.as_bytes()).unwrap();
        let completion = run_ready(complete_wpa2_ap_message4(
            &mut state,
            m4,
            m3.ptk(),
            &mut crypto,
        ))
        .unwrap();
        let (install, authorize) = completion.into_commands();
        assert!(matches!(
            install,
            Wpa2IoCommand::InstallKey(_)
        ));
        assert!(matches!(
            authorize,
            Wpa2IoCommand::SetPeerAuthorized {
                interface: Wpa2Interface::AccessPoint,
                peer: actual,
                authorized: true,
            } if actual == peer
        ));
        assert_eq!(state.phase(), Wpa2ApPhase::Authorized);
    }

    fn rsn() -> OwnedRsnIe<22> {
        OwnedRsnIe::try_copy(&[
            0x30, 20, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac,
            2, 0x80, 0,
        ])
        .unwrap()
    }

    fn run_ready<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("software backend unexpectedly returned Pending"),
        }
    }
}
