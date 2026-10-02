//! The IEEE 802.11 key derivation function with SHA-256/384/512.
//!
//! SOURCE: IEEE Std 802.11-2020 12.7.1.6.2 (KDF).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha384, Sha512};
use zeroize::Zeroize;

/// The IEEE 802.11 KDF-Hash-Length: HMAC blocks over a 16-bit
/// counter, the label, the context and the output length in bits.
macro_rules! kdf {
    ($name:ident, $hash:ty) => {
        pub(crate) fn $name(key: &[u8], label: &[u8], context: &[u8], output: &mut [u8]) {
            let bits = u16::try_from(output.len() * u8::BITS as usize)
                .expect("fixed suite KDF outputs fit the IEEE 16-bit length field");
            for (counter, chunk) in
                (1_u16..).zip(output.chunks_mut(<$hash as Digest>::output_size()))
            {
                let mut mac =
                    Hmac::<$hash>::new_from_slice(key).expect("HMAC accepts every key length");
                mac.update(&counter.to_le_bytes());
                mac.update(label);
                mac.update(context);
                mac.update(&bits.to_le_bytes());
                let mut block = mac.finalize().into_bytes();
                chunk.copy_from_slice(&block[..chunk.len()]);
                block.zeroize();
            }
        }
    };
}
kdf!(kdf_sha256, Sha256);
kdf!(kdf_sha384, Sha384);
kdf!(kdf_sha512, Sha512);
