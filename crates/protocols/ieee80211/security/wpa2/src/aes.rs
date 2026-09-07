//! Allocation-free RFC 3394 AES-128 key wrap and unwrap for WPA2 key data.

use ::aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{WPA2_KEY_DATA_CAPACITY, WPA2_UNWRAPPED_KEY_DATA_CAPACITY};

const RFC3394_IV: [u8; 8] = [0xa6; 8];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftwareAesKeyUnwrapError {
    InvalidLength,
    CapacityExceeded,
    IntegrityCheckFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftwareAesKeyWrapError {
    InvalidLength,
    CapacityExceeded,
}

/// Fixed, secret-bearing RFC 3394 ciphertext produced for an AP EAPOL-Key
/// frame. The complete buffer is cleared on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Wpa2WrappedKeyData {
    len: usize,
    bytes: [u8; WPA2_KEY_DATA_CAPACITY],
}

impl Wpa2WrappedKeyData {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2UnwrappedKeyDataCapacityError;

/// Fixed owned plaintext returned by an async key-unwrap backend.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Wpa2UnwrappedKeyData {
    len: usize,
    bytes: [u8; WPA2_UNWRAPPED_KEY_DATA_CAPACITY],
}

impl Wpa2UnwrappedKeyData {
    /// Copy plaintext produced by an external or test key-unwrap backend.
    ///
    /// The public constructor is required for implementations of
    /// [`AsyncWpa2KeyUnwrap`] outside this crate; keeping the fields private
    /// still prevents an invalid length from being fabricated.
    pub fn try_copy(bytes: &[u8]) -> Result<Self, Wpa2UnwrappedKeyDataCapacityError> {
        if bytes.len() > WPA2_UNWRAPPED_KEY_DATA_CAPACITY {
            return Err(Wpa2UnwrappedKeyDataCapacityError);
        }
        let mut owned = Self {
            len: bytes.len(),
            bytes: [0; WPA2_UNWRAPPED_KEY_DATA_CAPACITY],
        };
        owned.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(owned)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
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

/// Wrap a bounded WPA2 key-data plaintext using RFC 3394 and AES-128.
///
/// WPA2 Message 3 key data is already padded to a multiple of eight bytes by
/// the frame builder. Inputs outside the RFC 3394 domain fail before output
/// is exposed.
pub fn software_aes128_key_wrap(
    kek: &[u8; 16],
    plaintext: &[u8],
) -> Result<Wpa2WrappedKeyData, SoftwareAesKeyWrapError> {
    if plaintext.len() < 16 || !plaintext.len().is_multiple_of(8) {
        return Err(SoftwareAesKeyWrapError::InvalidLength);
    }
    let len = plaintext
        .len()
        .checked_add(8)
        .ok_or(SoftwareAesKeyWrapError::CapacityExceeded)?;
    if len > WPA2_KEY_DATA_CAPACITY {
        return Err(SoftwareAesKeyWrapError::CapacityExceeded);
    }

    let cipher = Aes128::new(GenericArray::from_slice(kek));
    let mut output = Wpa2WrappedKeyData {
        len,
        bytes: [0; WPA2_KEY_DATA_CAPACITY],
    };
    output.bytes[..8].copy_from_slice(&RFC3394_IV);
    output.bytes[8..len].copy_from_slice(plaintext);
    let blocks = plaintext.len() / 8;
    let mut block = GenericArray::default();

    for round in 0..=5_u64 {
        for index in 1..=blocks {
            block[..8].copy_from_slice(&output.bytes[..8]);
            let offset = 8 + (index - 1) * 8;
            block[8..].copy_from_slice(&output.bytes[offset..offset + 8]);
            cipher.encrypt_block(&mut block);
            let counter = (round * blocks as u64 + index as u64).to_be_bytes();
            for byte in 0..8 {
                output.bytes[byte] = block[byte] ^ counter[byte];
            }
            output.bytes[offset..offset + 8].copy_from_slice(&block[8..]);
        }
    }
    block.zeroize();
    Ok(output)
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
mod tests;
