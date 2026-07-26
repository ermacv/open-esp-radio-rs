//! Async WPA2 STA M1-to-M2 transition with fixed owned storage.

use crate::{
    wpa2::OwnedEapolFrame,
    wpa2_aes::AsyncWpa2KeyUnwrap,
    wpa2_crypto::{new_mic_job, Wpa2Ptk},
    wpa2_frames::{
        build_sta_action_frame, parse_gtk_key_data, OwnedAssociationSecurityIes, Wpa2EthernetFrame,
        Wpa2FrameError, WPA2_TX_EAPOL_CAPACITY, WPA2_TX_ETHERNET_CAPACITY,
    },
    wpa2_io::Wpa2KeyInstall,
    wpa2_state::{PtkContext, Wpa2StaAction, Wpa2StaState, Wpa2StateError},
    CryptoJobError,
};

/// Cryptography required before a STA can transmit WPA2 message 2.
///
/// Implementations own the scheduling boundary. A hardware implementation
/// should arm SHA work, return `Pending`, and resume from a completion IRQ. No
/// method may allocate, busy-poll, insert a delay, or wait on an RTOS object.
#[allow(async_fn_in_trait)]
pub trait AsyncWpa2StaCrypto {
    type Error;

    async fn derive_ptk(
        &mut self,
        pmk: &[u8; 32],
        context: PtkContext,
    ) -> Result<[u8; 48], Self::Error>;

    async fn hmac_sha1(
        &mut self,
        key: &[u8; 16],
        message_with_zeroed_mic: &[u8],
    ) -> Result<[u8; 20], Self::Error>;
}

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2StaMessage2Error<E> {
    State(Wpa2StateError),
    Frame(Wpa2FrameError),
    Crypto(E),
    UnexpectedAction,
}

pub struct Wpa2StaMessage2 {
    ptk: Wpa2Ptk,
    frame: Wpa2EthernetFrame<WPA2_TX_ETHERNET_CAPACITY>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Wpa2StaMessage4Error<S, A> {
    State(Wpa2StateError),
    Frame(Wpa2FrameError),
    MicJob(CryptoJobError),
    Sha1(S),
    KeyUnwrap(A),
    Message3MicMismatch,
    UnexpectedAction,
}

/// Complete set of owned commands produced by a valid WPA2 message 3.
///
/// The radio owner must execute these in order: pairwise key, group key, then
/// message 4. FIFO command ownership makes M4 impossible to overtake either
/// key installation without an RTOS synchronization primitive.
pub struct Wpa2StaMessage4 {
    pairwise_key: Wpa2KeyInstall,
    group_key: Wpa2KeyInstall,
    frame: Wpa2EthernetFrame<WPA2_TX_ETHERNET_CAPACITY>,
}

impl Wpa2StaMessage4 {
    pub fn into_parts(
        self,
    ) -> (
        Wpa2KeyInstall,
        Wpa2KeyInstall,
        Wpa2EthernetFrame<WPA2_TX_ETHERNET_CAPACITY>,
    ) {
        (self.pairwise_key, self.group_key, self.frame)
    }
}

impl Wpa2StaMessage2 {
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

/// Consume one owned M1 and produce a complete Ethernet/EAPOL M2.
///
/// The state is advanced to `AwaitingMessage3` only after PTK derivation
/// succeeds. The returned frame owns all bytes and can be moved into a bounded
/// radio-owner command queue without retaining any borrowed packet pointer.
pub async fn derive_wpa2_sta_message2<C, const N: usize, const R: usize>(
    state: &mut Wpa2StaState,
    message1: OwnedEapolFrame<N>,
    pmk: &[u8; 32],
    security_ies: &OwnedAssociationSecurityIes<R>,
    crypto: &mut C,
) -> Result<Wpa2StaMessage2, Wpa2StaMessage2Error<C::Error>>
where
    C: AsyncWpa2StaCrypto,
{
    let (ticket, context) = match state
        .on_frame(message1)
        .map_err(Wpa2StaMessage2Error::State)?
    {
        Wpa2StaAction::DerivePtk { ticket, context } => (ticket, context),
        _ => return Err(Wpa2StaMessage2Error::UnexpectedAction),
    };

    let ptk = crypto
        .derive_ptk(pmk, context)
        .await
        .map(Wpa2Ptk::from_bytes)
        .map_err(Wpa2StaMessage2Error::Crypto)?;
    let transmit = match state
        .complete_ptk::<N>(ticket, true)
        .map_err(Wpa2StaMessage2Error::State)?
    {
        Wpa2StaAction::Transmit(transmit) => transmit,
        _ => return Err(Wpa2StaMessage2Error::UnexpectedAction),
    };

    let mut eapol =
        build_sta_action_frame::<WPA2_TX_EAPOL_CAPACITY, R>(state, transmit, security_ies)
            .map_err(Wpa2StaMessage2Error::Frame)?;
    let mic = crypto
        .hmac_sha1(ptk.kck(), eapol.as_bytes())
        .await
        .map_err(Wpa2StaMessage2Error::Crypto)?;
    eapol.set_mic(
        mic[..16]
            .try_into()
            .expect("a SHA-1 tag always contains a 16-byte WPA2 MIC"),
    );
    let frame = Wpa2EthernetFrame::build(*state.local_address(), &eapol)
        .map_err(Wpa2StaMessage2Error::Frame)?;
    Ok(Wpa2StaMessage2 { ptk, frame })
}

/// Verify and decrypt one owned M3, validate its authenticator RSN/RSNXE
/// elements, advance the replay-safe STA state, and produce the two key-install
/// commands plus a MIC-authenticated M4.
pub async fn complete_wpa2_sta_message3<S, A, const N: usize, const R: usize>(
    state: &mut Wpa2StaState,
    message3: OwnedEapolFrame<N>,
    ptk: &Wpa2Ptk,
    security_ies: &OwnedAssociationSecurityIes<R>,
    authenticator_rsn_ie: &[u8],
    authenticator_rsnxe: &[u8],
    sha1: &mut S,
    aes: &mut A,
) -> Result<Wpa2StaMessage4, Wpa2StaMessage4Error<S::Error, A::Error>>
where
    S: AsyncWpa2StaCrypto,
    A: AsyncWpa2KeyUnwrap,
{
    let (mic_ticket, retained) = match state
        .on_frame(message3)
        .map_err(Wpa2StaMessage4Error::State)?
    {
        Wpa2StaAction::VerifyMessage3Mic { ticket, frame } => (ticket, frame),
        _ => return Err(Wpa2StaMessage4Error::UnexpectedAction),
    };
    let expected_mic = *retained.key_frame().mic();
    let mic_job = new_mic_job(ptk.kck(), &retained).map_err(Wpa2StaMessage4Error::MicJob)?;
    let actual_mic = sha1
        .hmac_sha1(ptk.kck(), mic_job.input())
        .await
        .map_err(Wpa2StaMessage4Error::Sha1)?;
    let valid_mic = constant_time_equal(&actual_mic[..16], &expected_mic);
    let action = state
        .complete_message3_mic(mic_ticket, retained, valid_mic)
        .map_err(Wpa2StaMessage4Error::State)?;
    if !valid_mic {
        return Err(Wpa2StaMessage4Error::Message3MicMismatch);
    }

    let (install_ticket, retained, gtk) = match action {
        Wpa2StaAction::DecryptMessage3KeyData { ticket, frame } => {
            let plain = match aes
                .unwrap_key_data(ptk.kek(), frame.key_frame().key_data())
                .await
            {
                Ok(plain) => plain,
                Err(error) => {
                    let _ = state.complete_key_data(ticket, frame, false);
                    return Err(Wpa2StaMessage4Error::KeyUnwrap(error));
                }
            };
            let gtk = match parse_gtk_key_data(
                plain.as_bytes(),
                authenticator_rsn_ie,
                authenticator_rsnxe,
            ) {
                Ok(gtk) => gtk,
                Err(error) => {
                    let _ = state.complete_key_data(ticket, frame, false);
                    return Err(Wpa2StaMessage4Error::Frame(error));
                }
            };
            match state
                .complete_key_data(ticket, frame, true)
                .map_err(Wpa2StaMessage4Error::State)?
            {
                Wpa2StaAction::InstallKeys { ticket, frame } => (ticket, frame, gtk),
                _ => return Err(Wpa2StaMessage4Error::UnexpectedAction),
            }
        }
        Wpa2StaAction::InstallKeys { ticket, frame } => {
            let gtk = match parse_gtk_key_data(
                frame.key_frame().key_data(),
                authenticator_rsn_ie,
                authenticator_rsnxe,
            ) {
                Ok(gtk) => gtk,
                Err(error) => {
                    let _ = state.complete_key_install::<N>(ticket, false);
                    return Err(Wpa2StaMessage4Error::Frame(error));
                }
            };
            (ticket, frame, gtk)
        }
        _ => return Err(Wpa2StaMessage4Error::UnexpectedAction),
    };

    let receive_sequence = *retained.key_frame().key_receive_sequence();
    let pairwise_key = Wpa2KeyInstall::pairwise(
        crate::wpa2::Wpa2Interface::Station,
        *state.peer(),
        [0; 8],
        ptk,
    );
    let group_key =
        Wpa2KeyInstall::group(crate::wpa2::Wpa2Interface::Station, &gtk, receive_sequence);
    let transmit = match state
        .complete_key_install::<N>(install_ticket, true)
        .map_err(Wpa2StaMessage4Error::State)?
    {
        Wpa2StaAction::Transmit(transmit) => transmit,
        _ => return Err(Wpa2StaMessage4Error::UnexpectedAction),
    };
    let mut eapol =
        build_sta_action_frame::<WPA2_TX_EAPOL_CAPACITY, R>(state, transmit, security_ies)
            .map_err(Wpa2StaMessage4Error::Frame)?;
    let mic = sha1
        .hmac_sha1(ptk.kck(), eapol.as_bytes())
        .await
        .map_err(Wpa2StaMessage4Error::Sha1)?;
    eapol.set_mic(
        mic[..16]
            .try_into()
            .expect("a SHA-1 tag always contains a 16-byte WPA2 MIC"),
    );
    let frame = Wpa2EthernetFrame::build(*state.local_address(), &eapol)
        .map_err(Wpa2StaMessage4Error::Frame)?;
    Ok(Wpa2StaMessage4 {
        pairwise_key,
        group_key,
        frame,
    })
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
    use super::*;
    use crate::{
        wpa2::{EapolKeyMessage, Wpa2Interface},
        wpa2_aes::Wpa2SoftwareAes,
        wpa2_frames::{Wpa2Gtk, Wpa2PlainKeyData, Wpa2TxFrame},
        wpa2_io::Wpa2KeyKind,
        wpa2_state::Wpa2StaPhase,
    };
    use aes::{
        cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit},
        Aes128,
    };

    struct Crypto;

    impl AsyncWpa2StaCrypto for Crypto {
        type Error = ();

        async fn derive_ptk(
            &mut self,
            _pmk: &[u8; 32],
            _context: PtkContext,
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
    fn m1_becomes_owned_m2_after_async_crypto() {
        let local = [1; 6];
        let peer = [2; 6];
        let mut state = Wpa2StaState::new(local, peer, [3; 32]).unwrap();
        let source = Wpa2TxFrame::<128>::message1(peer, 7, [4; 32]).unwrap();
        let message1: OwnedEapolFrame<128> =
            OwnedEapolFrame::try_copy(Wpa2Interface::Station, peer, source.as_bytes()).unwrap();
        let mut rsn = [0; 22];
        rsn[0] = 0x30;
        rsn[1] = 20;
        let rsn: crate::wpa2_frames::OwnedRsnIe<22> =
            crate::wpa2_frames::OwnedRsnIe::try_copy(&rsn).unwrap();
        let security_ies = OwnedAssociationSecurityIes::<22>::try_copy(&rsn, &[]).unwrap();

        let mut crypto = Crypto;
        let output = futures_lite_for_test(derive_wpa2_sta_message2(
            &mut state,
            message1,
            &[9; 32],
            &security_ies,
            &mut crypto,
        ))
        .unwrap();

        assert_eq!(state.phase(), Wpa2StaPhase::AwaitingMessage3);
        assert_eq!(output.ptk().temporal_key(), &[0x55; 16]);
        assert_eq!(output.frame().interface(), Wpa2Interface::Station);
        let eapol = &output.frame().as_bytes()[14..];
        assert_eq!(
            crate::wpa2::EapolKeyFrame::parse(eapol).unwrap().message(),
            EapolKeyMessage::PairwiseMessage2
        );
        assert_eq!(&eapol[81..97], &[0xa5; 16]);
    }

    #[test]
    fn valid_m3_becomes_ordered_keys_and_owned_m4() {
        let local = [1; 6];
        let peer = [2; 6];
        let authenticator_nonce = [4; 32];
        let mut state = Wpa2StaState::new(local, peer, [3; 32]).unwrap();
        let message1 = Wpa2TxFrame::<128>::message1(peer, 7, authenticator_nonce).unwrap();
        let message1: OwnedEapolFrame<128> =
            OwnedEapolFrame::try_copy(Wpa2Interface::Station, peer, message1.as_bytes()).unwrap();
        let ticket = match state.on_frame(message1).unwrap() {
            Wpa2StaAction::DerivePtk { ticket, .. } => ticket,
            _ => panic!("unexpected M1 action"),
        };
        assert!(matches!(
            state.complete_ptk::<128>(ticket, true).unwrap(),
            Wpa2StaAction::Transmit(_)
        ));

        let mut rsn = [0; 22];
        rsn[0] = 0x30;
        rsn[1] = 20;
        let rsn = crate::wpa2_frames::OwnedRsnIe::<22>::try_copy(&rsn).unwrap();
        let security_ies = OwnedAssociationSecurityIes::<22>::try_copy(&rsn, &[]).unwrap();
        let gtk = Wpa2Gtk::new(2, false, [0x77; 16]).unwrap();
        let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
        let ptk = Wpa2Ptk::from_bytes([0x55; 48]);
        let encrypted = key_wrap(ptk.kek(), plain.as_bytes());
        let mut message3 =
            Wpa2TxFrame::<256>::message3(peer, 8, authenticator_nonce, [9; 8], &encrypted).unwrap();
        message3.set_mic(&[0xa5; 16]);
        let message3: OwnedEapolFrame<256> =
            OwnedEapolFrame::try_copy(Wpa2Interface::Station, peer, message3.as_bytes()).unwrap();

        let mut sha1 = Crypto;
        let mut aes = Wpa2SoftwareAes::new();
        let completion = futures_lite_for_test(complete_wpa2_sta_message3(
            &mut state,
            message3,
            &ptk,
            &security_ies,
            rsn.as_bytes(),
            &[],
            &mut sha1,
            &mut aes,
        ))
        .unwrap();
        let (pairwise, group, message4) = completion.into_parts();
        assert_eq!(pairwise.kind(), Wpa2KeyKind::Pairwise);
        assert_eq!(pairwise.peer(), &peer);
        assert_eq!(pairwise.key().as_bytes(), &[0x55; 16]);
        assert_eq!(
            group.kind(),
            Wpa2KeyKind::Group {
                key_id: 2,
                transmit: false
            }
        );
        assert_eq!(group.receive_sequence(), &[9; 8]);
        assert_eq!(group.key().as_bytes(), &[0x77; 16]);
        assert_eq!(
            crate::wpa2::EapolKeyFrame::parse(&message4.as_bytes()[14..])
                .unwrap()
                .message(),
            EapolKeyMessage::PairwiseMessage4
        );
        assert_eq!(state.phase(), Wpa2StaPhase::Completed);
    }

    fn key_wrap(kek: &[u8; 16], plain: &[u8]) -> [u8; 56] {
        assert_eq!(plain.len(), 48);
        let cipher = Aes128::new(GenericArray::from_slice(kek));
        let mut accumulator = [0xa6; 8];
        let mut output = [0; 56];
        output[8..].copy_from_slice(plain);
        let mut block = GenericArray::default();
        for round in 0..=5_u64 {
            for index in 1..=6_usize {
                block[..8].copy_from_slice(&accumulator);
                let offset = index * 8;
                block[8..].copy_from_slice(&output[offset..offset + 8]);
                cipher.encrypt_block(&mut block);
                let counter = (round * 6 + index as u64).to_be_bytes();
                for byte in 0..8 {
                    accumulator[byte] = block[byte] ^ counter[byte];
                }
                output[offset..offset + 8].copy_from_slice(&block[8..]);
            }
        }
        output[..8].copy_from_slice(&accumulator);
        output
    }

    fn futures_lite_for_test<F: core::future::Future>(future: F) -> F::Output {
        use core::{
            pin::pin,
            task::{Context, Poll, Waker},
        };

        let mut future = pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("test crypto unexpectedly returned Pending"),
        }
    }
}
