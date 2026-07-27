//! Allocation-free AES-128 key unwrap for WPA2 group-key data.
//!
//! This is the RFC 3394 implementation moved from
//! `migration/esp32s31-hybrid-runtime/src/wpa2_aes.rs`.

use ::aes::{
    Aes128,
    cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray},
};
use zeroize::Zeroize;

use crate::WPA2_UNWRAPPED_KEY_DATA_CAPACITY;

const RFC3394_IV: [u8; 8] = [0xa6; 8];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftwareAesKeyUnwrapError {
    InvalidLength,
    CapacityExceeded,
    IntegrityCheckFailed,
}

/// Fixed owned plaintext returned by an async key-unwrap backend.
pub struct Wpa2UnwrappedKeyData {
    len: usize,
    bytes: [u8; WPA2_UNWRAPPED_KEY_DATA_CAPACITY],
}

impl Wpa2UnwrappedKeyData {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Drop for Wpa2UnwrappedKeyData {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// Async-capable boundary for the AES operation needed by WPA2 Message 3.
///
/// A hardware backend may resume from an interrupt. Implementations must not
/// retain either borrowed input after returning.
#[allow(async_fn_in_trait)]
pub trait AsyncWpa2KeyUnwrap {
    type Error;

    async fn unwrap_key_data(
        &mut self,
        kek: &[u8; 16],
        encrypted: &[u8],
    ) -> Result<Wpa2UnwrappedKeyData, Self::Error>;
}

/// Pure RustCrypto AES leaf with an explicit RFC 3394 input bound.
///
/// The returned future is ready on its first poll and performs no allocation,
/// hardware-status polling, delay, or OS call.
pub struct Wpa2SoftwareAes;

impl Wpa2SoftwareAes {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for Wpa2SoftwareAes {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncWpa2KeyUnwrap for Wpa2SoftwareAes {
    type Error = SoftwareAesKeyUnwrapError;

    async fn unwrap_key_data(
        &mut self,
        kek: &[u8; 16],
        encrypted: &[u8],
    ) -> Result<Wpa2UnwrappedKeyData, Self::Error> {
        software_aes128_key_unwrap(kek, encrypted)
    }
}

pub fn software_aes128_key_unwrap(
    kek: &[u8; 16],
    encrypted: &[u8],
) -> Result<Wpa2UnwrappedKeyData, SoftwareAesKeyUnwrapError> {
    if encrypted.len() < 24 || !encrypted.len().is_multiple_of(8) {
        return Err(SoftwareAesKeyUnwrapError::InvalidLength);
    }
    let len = encrypted.len() - 8;
    if len > WPA2_UNWRAPPED_KEY_DATA_CAPACITY {
        return Err(SoftwareAesKeyUnwrapError::CapacityExceeded);
    }

    let cipher = Aes128::new(GenericArray::from_slice(kek));
    let mut accumulator = [0; 8];
    accumulator.copy_from_slice(&encrypted[..8]);
    let mut output = Wpa2UnwrappedKeyData {
        len,
        bytes: [0; WPA2_UNWRAPPED_KEY_DATA_CAPACITY],
    };
    output.bytes[..len].copy_from_slice(&encrypted[8..]);
    let blocks = len / 8;
    let mut block = GenericArray::default();

    for round in (0..=5_u64).rev() {
        for index in (1..=blocks).rev() {
            let counter = (round * blocks as u64 + index as u64).to_be_bytes();
            for byte in 0..8 {
                block[byte] = accumulator[byte] ^ counter[byte];
            }
            let offset = (index - 1) * 8;
            block[8..].copy_from_slice(&output.bytes[offset..offset + 8]);
            cipher.decrypt_block(&mut block);
            accumulator.copy_from_slice(&block[..8]);
            output.bytes[offset..offset + 8].copy_from_slice(&block[8..]);
        }
    }
    block.zeroize();

    let mut difference = 0;
    for (actual, expected) in accumulator.iter().zip(RFC3394_IV) {
        difference |= actual ^ expected;
    }
    accumulator.zeroize();
    if difference != 0 {
        return Err(SoftwareAesKeyUnwrapError::IntegrityCheckFailed);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_rfc3394_vector_and_rejects_changed_integrity() {
        let kek = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let encrypted = [
            0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a,
            0x7b, 0x82, 0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
        ];
        let expected = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let plain = software_aes128_key_unwrap(&kek, &encrypted).unwrap();
        assert_eq!(plain.as_bytes(), &expected);

        let mut changed = encrypted;
        changed[23] ^= 1;
        assert_eq!(
            software_aes128_key_unwrap(&kek, &changed).err(),
            Some(SoftwareAesKeyUnwrapError::IntegrityCheckFailed)
        );
    }

    #[test]
    fn rejects_non_rfc3394_lengths() {
        assert_eq!(
            software_aes128_key_unwrap(&[0; 16], &[0; 16]).err(),
            Some(SoftwareAesKeyUnwrapError::InvalidLength)
        );
        assert_eq!(
            software_aes128_key_unwrap(&[0; 16], &[0; 25]).err(),
            Some(SoftwareAesKeyUnwrapError::InvalidLength)
        );
    }
}
