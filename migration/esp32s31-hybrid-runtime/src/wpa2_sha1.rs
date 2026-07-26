//! WPA2 SHA-1 primitives over an interrupt-driven streaming digest.

use core::sync::atomic::{compiler_fence, Ordering};

use crate::{wpa2_sta_async::AsyncWpa2StaCrypto, wpa2_state::PtkContext};

const SHA1_BLOCK_LEN: usize = 64;
const SHA1_DIGEST_LEN: usize = 20;
const WPA2_PRF_LABEL: &[u8] = b"Pairwise key expansion";
const WPA2_PTK_CONTEXT_LEN: usize = 76;
const WPA2_PRF_MESSAGE_LEN: usize = WPA2_PRF_LABEL.len() + 1 + WPA2_PTK_CONTEXT_LEN + 1;

/// Largest SHA-1 message used by WPA2 HMAC in this crate: one 64-byte HMAC
/// pad followed by a maximum 512-byte EAPOL frame.
pub const WPA2_SOFTWARE_SHA1_MAX_MESSAGE_LEN: usize = 576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftwareSha1Error {
    MessageTooLong,
}

/// Pure CPU SHA-1 with an explicit WPA2-sized execution bound.
///
/// It performs no hardware-status read, wait, wake, allocation, delay, or
/// RTOS operation. The returned future is ready on its first poll. This is
/// intentionally preferred on ESP32-S31 because SHA-1 mode does not complete
/// through the otherwise working SHA DMA handshake, while WPA2 handshake
/// messages have a small fixed maximum size.
pub struct Wpa2SoftwareSha1;

impl Wpa2SoftwareSha1 {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for Wpa2SoftwareSha1 {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal streaming boundary required from a strict SHA-1 implementation.
///
/// A conforming hardware implementation processes all `parts` as one SHA-1
/// message. It may copy them into fixed DMA storage, but it must wait only for
/// completion interrupts. In particular, wrapping the CPU `Sha1Context`
/// work-queue handle is not conforming because that backend self-wakes to poll
/// a peripheral without a completion interrupt.
#[allow(async_fn_in_trait)]
pub trait AsyncSha1 {
    type Error;

    async fn digest(&mut self, parts: &[&[u8]]) -> Result<[u8; SHA1_DIGEST_LEN], Self::Error>;
}

impl AsyncSha1 for Wpa2SoftwareSha1 {
    type Error = SoftwareSha1Error;

    async fn digest(&mut self, parts: &[&[u8]]) -> Result<[u8; 20], Self::Error> {
        software_sha1_digest_parts(parts)
    }
}

/// Synchronous bounded leaf used by cooperative Rust-owned algorithms.
///
/// The leaf performs a finite amount of CPU work for a bounded message. It
/// does not inspect hardware status, wait, allocate, or wake the executor.
/// Callers implementing expensive algorithms such as PBKDF2 must impose their
/// own per-poll work budget around calls to this function.
pub(crate) fn software_sha1_digest_parts(
    parts: &[&[u8]],
) -> Result<[u8; SHA1_DIGEST_LEN], SoftwareSha1Error> {
    let mut message_len = 0_usize;
    for part in parts {
        message_len = message_len
            .checked_add(part.len())
            .filter(|length| *length <= WPA2_SOFTWARE_SHA1_MAX_MESSAGE_LEN)
            .ok_or(SoftwareSha1Error::MessageTooLong)?;
    }

    let mut state = [
        0x6745_2301,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    let mut block = [0_u8; SHA1_BLOCK_LEN];
    let mut used = 0;

    for part in parts {
        let mut remaining = *part;
        while !remaining.is_empty() {
            let copied = core::cmp::min(SHA1_BLOCK_LEN - used, remaining.len());
            block[used..used + copied].copy_from_slice(&remaining[..copied]);
            used += copied;
            remaining = &remaining[copied..];
            if used == SHA1_BLOCK_LEN {
                compress_sha1(&mut state, &block);
                block.fill(0);
                used = 0;
            }
        }
    }

    block[used] = 0x80;
    used += 1;
    if used > 56 {
        block[used..].fill(0);
        compress_sha1(&mut state, &block);
        block.fill(0);
    } else {
        block[used..56].fill(0);
    }
    block[56..].copy_from_slice(&((message_len as u64) * 8).to_be_bytes());
    compress_sha1(&mut state, &block);

    let mut digest = [0; SHA1_DIGEST_LEN];
    for (word, output) in state.iter().zip(digest.chunks_exact_mut(4)) {
        output.copy_from_slice(&word.to_be_bytes());
    }
    zeroize(&mut block);
    zeroize_words(&mut state);
    Ok(digest)
}

/// Allocation-free HMAC-SHA1 and WPA2 PRF-384 built on [`AsyncSha1`].
pub struct Wpa2Sha1Crypto<S> {
    sha1: S,
}

impl<S> Wpa2Sha1Crypto<S> {
    pub const fn new(sha1: S) -> Self {
        Self { sha1 }
    }

    pub const fn backend(&self) -> &S {
        &self.sha1
    }

    pub fn backend_mut(&mut self) -> &mut S {
        &mut self.sha1
    }

    pub fn into_backend(self) -> S {
        self.sha1
    }
}

impl<S> Wpa2Sha1Crypto<S>
where
    S: AsyncSha1,
{
    /// Compute HMAC-SHA1 without concatenating into heap storage.
    async fn hmac_sha1_bounded(
        &mut self,
        key: &[u8],
        message: &[u8],
    ) -> Result<[u8; SHA1_DIGEST_LEN], S::Error> {
        // This private helper is reached only with the fixed 32-byte PMK or
        // 16-byte KCK. Generic HMAC long-key reduction is intentionally not
        // exposed by the WPA2-only type.

        let mut inner_pad = [0x36; SHA1_BLOCK_LEN];
        let mut outer_pad = [0x5c; SHA1_BLOCK_LEN];
        for (index, byte) in key.iter().enumerate() {
            inner_pad[index] ^= byte;
            outer_pad[index] ^= byte;
        }

        let inner = match self.sha1.digest(&[&inner_pad, message]).await {
            Ok(inner) => inner,
            Err(error) => {
                zeroize(&mut inner_pad);
                zeroize(&mut outer_pad);
                return Err(error);
            }
        };
        let result = self.sha1.digest(&[&outer_pad, &inner]).await;

        zeroize(&mut inner_pad);
        zeroize(&mut outer_pad);
        result
    }

    async fn derive_ptk_bytes(
        &mut self,
        pmk: &[u8; 32],
        context: PtkContext,
    ) -> Result<[u8; 48], S::Error> {
        let mut canonical = [0; WPA2_PTK_CONTEXT_LEN];
        let (first_address, second_address) =
            ordered(&context.authenticator_address, &context.supplicant_address);
        canonical[..6].copy_from_slice(first_address);
        canonical[6..12].copy_from_slice(second_address);
        let (first_nonce, second_nonce) =
            ordered(&context.authenticator_nonce, &context.supplicant_nonce);
        canonical[12..44].copy_from_slice(first_nonce);
        canonical[44..76].copy_from_slice(second_nonce);

        let mut message = [0; WPA2_PRF_MESSAGE_LEN];
        message[..WPA2_PRF_LABEL.len()].copy_from_slice(WPA2_PRF_LABEL);
        let context_start = WPA2_PRF_LABEL.len() + 1;
        message[context_start..context_start + canonical.len()].copy_from_slice(&canonical);

        let mut ptk = [0; 48];
        let mut written = 0;
        let mut counter = 0_u8;
        while written < ptk.len() {
            message[WPA2_PRF_MESSAGE_LEN - 1] = counter;
            let block = match self.hmac_sha1_bounded(pmk, &message).await {
                Ok(block) => block,
                Err(error) => {
                    zeroize(&mut canonical);
                    zeroize(&mut message);
                    zeroize(&mut ptk);
                    return Err(error);
                }
            };
            let count = core::cmp::min(block.len(), ptk.len() - written);
            ptk[written..written + count].copy_from_slice(&block[..count]);
            written += count;
            counter = counter.wrapping_add(1);
        }

        zeroize(&mut canonical);
        zeroize(&mut message);
        Ok(ptk)
    }
}

impl<S> AsyncWpa2StaCrypto for Wpa2Sha1Crypto<S>
where
    S: AsyncSha1,
{
    type Error = S::Error;

    async fn derive_ptk(
        &mut self,
        pmk: &[u8; 32],
        context: PtkContext,
    ) -> Result<[u8; 48], Self::Error> {
        self.derive_ptk_bytes(pmk, context).await
    }

    async fn hmac_sha1(
        &mut self,
        key: &[u8; 16],
        message_with_zeroed_mic: &[u8],
    ) -> Result<[u8; 20], Self::Error> {
        self.hmac_sha1_bounded(key, message_with_zeroed_mic).await
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

fn zeroize(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

fn zeroize_words(words: &mut [u32]) {
    for word in words {
        unsafe { core::ptr::write_volatile(word, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

fn compress_sha1(state: &mut [u32; 5], block: &[u8; SHA1_BLOCK_LEN]) {
    let mut schedule = [0_u32; 80];
    for (word, bytes) in schedule[..16].iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes(bytes.try_into().expect("SHA-1 words contain four bytes"));
    }
    for index in 16..80 {
        schedule[index] = (schedule[index - 3]
            ^ schedule[index - 8]
            ^ schedule[index - 14]
            ^ schedule[index - 16])
            .rotate_left(1);
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    for (index, word) in schedule.iter().enumerate() {
        let (function, constant) = match index {
            0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
            20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(function)
            .wrapping_add(e)
            .wrapping_add(constant)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    zeroize_words(&mut schedule);
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use sha1::{Digest, Sha1};

    use super::*;

    struct SoftwareSha1;

    impl AsyncSha1 for SoftwareSha1 {
        type Error = ();

        async fn digest(&mut self, parts: &[&[u8]]) -> Result<[u8; 20], Self::Error> {
            let mut hasher = Sha1::new();
            for part in parts {
                hasher.update(part);
            }
            Ok(hasher.finalize().into())
        }
    }

    #[test]
    fn hmac_sha1_matches_rfc_2202_case_one() {
        let mut crypto = Wpa2Sha1Crypto::new(SoftwareSha1);
        let actual = run_ready(crypto.hmac_sha1_bounded(&[0x0b; 20], b"Hi There")).unwrap();
        assert_eq!(
            actual,
            [
                0xb6, 0x17, 0x31, 0x86, 0x55, 0x05, 0x72, 0x64, 0xe2, 0x8b, 0xc0, 0xb6, 0xfb, 0x37,
                0x8c, 0x8e, 0xf1, 0x46, 0xbe, 0x00,
            ]
        );
    }

    #[test]
    fn bounded_software_sha1_streams_parts_and_rejects_oversize() {
        let mut sha1 = Wpa2SoftwareSha1::new();
        let digest = run_ready(sha1.digest(&[b"a", b"", b"bc"])).unwrap();
        assert_eq!(
            digest,
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
            ]
        );
        let oversized = [0; WPA2_SOFTWARE_SHA1_MAX_MESSAGE_LEN + 1];
        assert_eq!(
            run_ready(sha1.digest(&[&oversized])),
            Err(SoftwareSha1Error::MessageTooLong)
        );
    }

    #[test]
    fn wpa2_software_sha1_drives_hmac_and_ptk() {
        let mut crypto = Wpa2Sha1Crypto::new(Wpa2SoftwareSha1::new());
        let actual = run_ready(crypto.hmac_sha1_bounded(&[0x0b; 20], b"Hi There")).unwrap();
        assert_eq!(
            actual,
            [
                0xb6, 0x17, 0x31, 0x86, 0x55, 0x05, 0x72, 0x64, 0xe2, 0x8b, 0xc0, 0xb6, 0xfb, 0x37,
                0x8c, 0x8e, 0xf1, 0x46, 0xbe, 0x00,
            ]
        );
    }

    #[test]
    fn wpa2_prf_384_matches_independent_vector() {
        let mut crypto = Wpa2Sha1Crypto::new(SoftwareSha1);
        let context = PtkContext {
            authenticator_address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
            supplicant_address: [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb],
            authenticator_nonce: core::array::from_fn(|index| index as u8 + 32),
            supplicant_nonce: core::array::from_fn(|index| index as u8 + 64),
        };
        let pmk = core::array::from_fn(|index| index as u8);
        let actual = run_ready(crypto.derive_ptk_bytes(&pmk, context)).unwrap();
        assert_eq!(
            actual,
            [
                0x35, 0x07, 0x82, 0xa5, 0x49, 0x8c, 0x27, 0x32, 0x15, 0xbf, 0x37, 0x70, 0x79, 0xe3,
                0x65, 0x0f, 0x63, 0x13, 0xd9, 0x26, 0xdb, 0xe9, 0xed, 0x87, 0x53, 0xa6, 0x0f, 0x1b,
                0x6e, 0x62, 0x25, 0xea, 0x5c, 0xbe, 0xca, 0x83, 0xd7, 0xbb, 0xa7, 0x6c, 0x9e, 0x6d,
                0x02, 0xa8, 0x48, 0xd1, 0xe5, 0x5f,
            ]
        );
    }

    fn run_ready<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("software SHA-1 unexpectedly returned Pending"),
        }
    }
}
