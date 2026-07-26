//! Fixed owned crypto jobs used by the WPA2-Personal state machines.

use core::sync::atomic::{compiler_fence, Ordering};

use crate::{
    crypto::{CryptoJob, CryptoJobError, CryptoOperation},
    wpa2::OwnedEapolFrame,
    wpa2_frames::{Wpa2PlainKeyData, Wpa2TxFrame},
    wpa2_state::PtkContext,
};

pub const WPA2_PTK_CONTEXT_LEN: usize = 76;
pub const WPA2_PTK_LEN: usize = 48;
pub const WPA2_KCK_LEN: usize = 16;
pub const WPA2_KEK_LEN: usize = 16;
pub const WPA2_TK_LEN: usize = 16;
pub const WPA2_MIC_OUTPUT_LEN: usize = 20;
pub const WPA2_KEY_DATA_CAPACITY: usize = 512;
pub const WPA2_UNWRAPPED_KEY_DATA_CAPACITY: usize = WPA2_KEY_DATA_CAPACITY - 8;

const EAPOL_MIC_START: usize = 81;
const EAPOL_MIC_END: usize = EAPOL_MIC_START + 16;

pub type Wpa2PtkJob = CryptoJob<32, WPA2_PTK_CONTEXT_LEN, WPA2_PTK_LEN>;
pub type Wpa2MicJob<const N: usize> = CryptoJob<WPA2_KCK_LEN, N, WPA2_MIC_OUTPUT_LEN>;
pub type Wpa2KeyDataJob =
    CryptoJob<WPA2_KEK_LEN, WPA2_KEY_DATA_CAPACITY, WPA2_UNWRAPPED_KEY_DATA_CAPACITY>;
pub type Wpa2KeyDataWrapJob =
    CryptoJob<WPA2_KEK_LEN, WPA2_UNWRAPPED_KEY_DATA_CAPACITY, WPA2_KEY_DATA_CAPACITY>;

pub fn new_ptk_job(pmk: &[u8; 32], context: PtkContext) -> Result<Wpa2PtkJob, CryptoJobError> {
    let mut input = [0; WPA2_PTK_CONTEXT_LEN];
    let (first_address, second_address) =
        ordered(&context.authenticator_address, &context.supplicant_address);
    input[..6].copy_from_slice(first_address);
    input[6..12].copy_from_slice(second_address);
    let (first_nonce, second_nonce) =
        ordered(&context.authenticator_nonce, &context.supplicant_nonce);
    input[12..44].copy_from_slice(first_nonce);
    input[44..76].copy_from_slice(second_nonce);
    Wpa2PtkJob::new(CryptoOperation::Wpa2PtkSha1, pmk, &[], &input, WPA2_PTK_LEN)
}

pub fn new_mic_job<const N: usize>(
    kck: &[u8; WPA2_KCK_LEN],
    frame: &OwnedEapolFrame<N>,
) -> Result<Wpa2MicJob<N>, CryptoJobError> {
    new_mic_job_from_bytes(kck, frame.as_bytes())
}

pub fn new_tx_mic_job<const N: usize>(
    kck: &[u8; WPA2_KCK_LEN],
    frame: &Wpa2TxFrame<N>,
) -> Result<Wpa2MicJob<N>, CryptoJobError> {
    new_mic_job_from_bytes(kck, frame.as_bytes())
}

fn new_mic_job_from_bytes<const N: usize>(
    kck: &[u8; WPA2_KCK_LEN],
    bytes: &[u8],
) -> Result<Wpa2MicJob<N>, CryptoJobError> {
    if bytes.len() > N || bytes.len() < EAPOL_MIC_END {
        return Err(CryptoJobError::InvalidInputLength);
    }
    let mut input = [0; N];
    input[..bytes.len()].copy_from_slice(bytes);
    input[EAPOL_MIC_START..EAPOL_MIC_END].fill(0);
    Wpa2MicJob::new(
        CryptoOperation::HmacSha1,
        kck,
        &[],
        &input[..bytes.len()],
        WPA2_MIC_OUTPUT_LEN,
    )
}

pub fn new_key_data_wrap_job<const N: usize>(
    kek: &[u8; WPA2_KEK_LEN],
    plain_key_data: &Wpa2PlainKeyData<N>,
) -> Result<Wpa2KeyDataWrapJob, CryptoJobError> {
    let input = plain_key_data.as_bytes();
    Wpa2KeyDataWrapJob::new(
        CryptoOperation::Aes128KeyWrap,
        kek,
        &[],
        input,
        input
            .len()
            .checked_add(8)
            .ok_or(CryptoJobError::InvalidOutputLength)?,
    )
}

pub fn new_key_data_job(
    kek: &[u8; WPA2_KEK_LEN],
    encrypted_key_data: &[u8],
) -> Result<Wpa2KeyDataJob, CryptoJobError> {
    Wpa2KeyDataJob::new(
        CryptoOperation::Aes128KeyUnwrap,
        kek,
        &[],
        encrypted_key_data,
        encrypted_key_data
            .len()
            .checked_sub(8)
            .ok_or(CryptoJobError::InvalidInputLength)?,
    )
}

pub fn verify_mic<const N: usize>(
    job: &Wpa2MicJob<N>,
    expected: &[u8; 16],
) -> Result<bool, CryptoJobError> {
    let result = job.result()?;
    let mut difference = 0_u8;
    for (actual, expected) in result[..16].iter().zip(expected) {
        difference |= actual ^ expected;
    }
    Ok(difference == 0)
}

/// Persistent WPA2-CCMP PTK split into KCK, KEK, and TK.
pub struct Wpa2Ptk {
    bytes: [u8; WPA2_PTK_LEN],
}

impl Wpa2Ptk {
    pub const fn from_bytes(bytes: [u8; WPA2_PTK_LEN]) -> Self {
        Self { bytes }
    }

    pub fn from_job(job: &Wpa2PtkJob) -> Result<Self, CryptoJobError> {
        let result = job.result()?;
        let mut bytes = [0; WPA2_PTK_LEN];
        bytes.copy_from_slice(result);
        Ok(Self { bytes })
    }

    pub fn kck(&self) -> &[u8; WPA2_KCK_LEN] {
        self.bytes[..WPA2_KCK_LEN]
            .try_into()
            .expect("fixed WPA2 KCK range")
    }

    pub fn kek(&self) -> &[u8; WPA2_KEK_LEN] {
        self.bytes[WPA2_KCK_LEN..WPA2_KCK_LEN + WPA2_KEK_LEN]
            .try_into()
            .expect("fixed WPA2 KEK range")
    }

    pub fn temporal_key(&self) -> &[u8; WPA2_TK_LEN] {
        self.bytes[WPA2_KCK_LEN + WPA2_KEK_LEN..]
            .try_into()
            .expect("fixed WPA2 TK range")
    }
}

impl Drop for Wpa2Ptk {
    fn drop(&mut self) {
        for byte in &mut self.bytes {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }
}

fn ordered<'a, const N: usize>(
    left: &'a [u8; N],
    right: &'a [u8; N],
) -> (&'a [u8; N], &'a [u8; N]) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wpa2::{
        Wpa2Interface, EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY,
        RSN_KEY_DESCRIPTOR_TYPE,
    };
    use crate::wpa2_frames::{OwnedRsnIe, Wpa2Gtk, Wpa2PlainKeyData, Wpa2TxFrame};

    #[test]
    fn ptk_job_canonicalizes_addresses_and_nonces() {
        let context = PtkContext {
            authenticator_address: [9; 6],
            supplicant_address: [1; 6],
            authenticator_nonce: [8; 32],
            supplicant_nonce: [2; 32],
        };
        let job = new_ptk_job(&[7; 32], context).unwrap();
        assert_eq!(job.operation(), CryptoOperation::Wpa2PtkSha1);
        assert_eq!(&job.input()[..6], &[1; 6]);
        assert_eq!(&job.input()[6..12], &[9; 6]);
        assert_eq!(&job.input()[12..44], &[2; 32]);
        assert_eq!(&job.input()[44..76], &[8; 32]);
    }

    #[test]
    fn mic_job_owns_frame_with_zeroed_mic_field() {
        let mut bytes = [0; EAPOL_KEY_PACKET_LEN];
        bytes[0] = 2;
        bytes[1] = EAPOL_PACKET_TYPE_KEY;
        bytes[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
        bytes[4] = RSN_KEY_DESCRIPTOR_TYPE;
        bytes[5..7].copy_from_slice(&(2_u16 | (1 << 3) | (1 << 8)).to_be_bytes());
        bytes[EAPOL_MIC_START..EAPOL_MIC_END].fill(0xa5);
        let frame: OwnedEapolFrame<EAPOL_KEY_PACKET_LEN> =
            OwnedEapolFrame::try_copy(Wpa2Interface::AccessPoint, [1; 6], &bytes).unwrap();
        let job = new_mic_job(&[3; 16], &frame).unwrap();
        assert_eq!(job.operation(), CryptoOperation::HmacSha1);
        assert_eq!(&job.input()[EAPOL_MIC_START..EAPOL_MIC_END], &[0; 16]);
        assert_eq!(frame.key_frame().mic(), &[0xa5; 16]);
    }

    #[test]
    fn key_unwrap_job_has_rfc3394_lengths() {
        let encrypted = [4; 24];
        let mut job = new_key_data_job(&[5; 16], &encrypted).unwrap();
        assert_eq!(job.operation(), CryptoOperation::Aes128KeyUnwrap);
        assert_eq!(job.input(), &encrypted);
        assert_eq!(job.output_buffer().len(), 16);
    }

    #[test]
    fn key_wrap_and_tx_mic_jobs_own_their_inputs() {
        let mut rsn = [0; 22];
        rsn[0] = 0x30;
        rsn[1] = 20;
        let rsn = OwnedRsnIe::<22>::try_copy(&rsn).unwrap();
        let gtk = Wpa2Gtk::new(1, false, [7; 16]).unwrap();
        let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
        let mut wrap = new_key_data_wrap_job(&[8; 16], &plain).unwrap();
        assert_eq!(wrap.operation(), CryptoOperation::Aes128KeyWrap);
        assert_eq!(wrap.input(), plain.as_bytes());
        assert_eq!(wrap.output_buffer().len(), plain.as_bytes().len() + 8);

        let mut tx = Wpa2TxFrame::<128>::message4([1; 6], 9).unwrap();
        tx.set_mic(&[0xa5; 16]);
        let mic = new_tx_mic_job(&[3; 16], &tx).unwrap();
        assert_eq!(&mic.input()[EAPOL_MIC_START..EAPOL_MIC_END], &[0; 16]);
        assert_eq!(tx.key_frame().mic(), &[0xa5; 16]);
    }
}
