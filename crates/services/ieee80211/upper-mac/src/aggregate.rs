//! A service's A-MPDUs over the port: whether the port sends them
//! ([`PortAggregation`]) and the subframes of one aggregate
//! ([`AmpduSubframes`]).
//!
//! A service decides which queued frames one aggregate carries
//! (`oer-ieee80211-upper-mac`'s `AmpduLimits`) and encodes each into the
//! subframes, as its role addresses and protects it; the subframes then
//! make the request and the frames its client sends.

use core::future::Future;

use oer_ieee80211_lower_mac::{AmpduCapabilities, Ieee80211Instant, KeySelector, LowerMacAmpdu};
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_upper_mac::{AmpduRequest, TxReport, TxRequest};

use crate::{
    AmpduFrames,
    client::{PortClient, PortClientEnv, PortClientError, PortError},
    queue::{PORT_MPDU_CAPACITY, PORT_TX_QUEUE},
};

const FCS_LEN: u16 = 4;
const CCMP_MIC_LEN: u16 = 8;

/// How a client's port sends A-MPDUs, which its integrator names once in
/// [`PortClientEnv::Aggregation`].
pub trait PortAggregation<X: PortClientEnv + ?Sized> {
    /// The port's A-MPDU capabilities; `None` sends every frame alone.
    fn capabilities(port: &X::Port) -> Option<AmpduCapabilities>;

    /// Send one A-MPDU through `client`.
    fn send<const EXCHANGES: usize, const RX: usize>(
        client: &mut PortClient<'_, X, EXCHANGES, RX>,
        frames: AmpduFrames<'_>,
        request: TxRequest,
    ) -> impl Future<Output = Result<TxReport, PortClientError<PortError<X>>>>
    where
        X: Sized;
}

/// A port without [`LowerMacAmpdu`]: every frame goes out alone, Block Ack
/// agreements notwithstanding.
pub struct NoAggregation;

impl<X: PortClientEnv + ?Sized> PortAggregation<X> for NoAggregation {
    fn capabilities(_port: &X::Port) -> Option<AmpduCapabilities> {
        None
    }

    async fn send<const EXCHANGES: usize, const RX: usize>(
        _client: &mut PortClient<'_, X, EXCHANGES, RX>,
        _frames: AmpduFrames<'_>,
        _request: TxRequest,
    ) -> Result<TxReport, PortClientError<PortError<X>>>
    where
        X: Sized,
    {
        Err(PortClientError::AggregationUnsupported)
    }
}

/// A port with [`LowerMacAmpdu`]: aggregates go through its A-MPDU
/// attempts.
pub struct PortAmpduAggregation;

impl<X: PortClientEnv + ?Sized> PortAggregation<X> for PortAmpduAggregation
where
    X::Port: LowerMacAmpdu,
{
    fn capabilities(port: &X::Port) -> Option<AmpduCapabilities> {
        Some(port.ampdu_capabilities())
    }

    async fn send<const EXCHANGES: usize, const RX: usize>(
        client: &mut PortClient<'_, X, EXCHANGES, RX>,
        frames: AmpduFrames<'_>,
        request: TxRequest,
    ) -> Result<TxReport, PortClientError<PortError<X>>>
    where
        X: Sized,
    {
        client.transmit_ampdu(frames, request).await
    }
}

/// The encoded subframes of one A-MPDU, at most [`PORT_TX_QUEUE`], each
/// from its MAC header to the end of its body; every subframe is under the
/// key of the last one pushed.
pub struct AmpduSubframes {
    mpdus: [[u8; PORT_MPDU_CAPACITY]; PORT_TX_QUEUE],
    lengths: [usize; PORT_TX_QUEUE],
    count: usize,
    key: KeySelector,
}

impl Default for AmpduSubframes {
    fn default() -> Self {
        Self::new()
    }
}

impl AmpduSubframes {
    pub const fn new() -> Self {
        Self {
            mpdus: [[0; PORT_MPDU_CAPACITY]; PORT_TX_QUEUE],
            lengths: [0; PORT_TX_QUEUE],
            count: 0,
            key: KeySelector::Plaintext,
        }
    }

    /// Begin another aggregate.
    pub fn clear(&mut self) {
        self.count = 0;
        self.key = KeySelector::Plaintext;
    }

    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Encode the next subframe with `encode`, which writes it into the
    /// buffer it gets and returns its length and key; `Ok(false)` when
    /// every subframe is taken, before `encode` runs.
    pub fn push<E>(
        &mut self,
        encode: impl FnOnce(&mut [u8]) -> Result<(usize, KeySelector), E>,
    ) -> Result<bool, E> {
        let Some(buffer) = self.mpdus.get_mut(self.count) else {
            return Ok(false);
        };
        let (length, key) = encode(buffer)?;
        self.lengths[self.count] = length;
        self.key = key;
        self.count += 1;
        Ok(true)
    }

    /// The request of the aggregate: `tid` from `first_sequence`, its
    /// lifetime from `committed_at`, under an operational agreement; each
    /// subframe's on-air length adds its FCS and, under a key, its MIC.
    /// `None` for no subframe.
    pub fn request(
        &self,
        tid: u8,
        first_sequence: SequenceNumber,
        committed_at: Ieee80211Instant,
    ) -> Option<AmpduRequest> {
        let trailer = FCS_LEN
            + if matches!(self.key, KeySelector::Key(_)) {
                CCMP_MIC_LEN
            } else {
                0
            };
        let mut on_air = [0_u16; PORT_TX_QUEUE];
        for (on_air, length) in on_air.iter_mut().zip(&self.lengths[..self.count]) {
            *on_air = *length as u16 + trailer;
        }
        AmpduRequest::new(
            tid,
            first_sequence,
            &on_air[..self.count],
            committed_at,
            true,
        )
    }

    /// The frames the client sends, their slices in `slices`, to a
    /// recipient of `min_mpdu_start_spacing` (IEEE encoding 0-7).
    pub fn frames<'a>(
        &'a self,
        slices: &'a mut [&'a [u8]; PORT_TX_QUEUE],
        min_mpdu_start_spacing: u8,
    ) -> AmpduFrames<'a> {
        for ((slice, mpdu), length) in slices.iter_mut().zip(&self.mpdus).zip(&self.lengths) {
            *slice = &mpdu[..*length];
        }
        AmpduFrames {
            subframes: &slices[..self.count],
            key: self.key,
            min_mpdu_start_spacing,
        }
    }
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_lower_mac::KeyHandle;

    use super::*;

    #[test]
    fn subframes_make_the_request_and_frames_under_the_last_key() {
        let mut subframes = AmpduSubframes::new();
        let key = KeySelector::Key(KeyHandle(3));
        for length in [40, 60] {
            assert_eq!(
                subframes.push(|buffer| {
                    buffer[..length].fill(length as u8);
                    Ok::<_, ()>((length, key))
                }),
                Ok(true)
            );
        }
        let request = subframes
            .request(0, SequenceNumber::ZERO, Ieee80211Instant::from_micros(0))
            .unwrap();
        // FCS and MIC on air.
        assert_eq!((request.mpdu_length(0), request.mpdu_length(1)), (52, 72));
        assert_eq!(request.subframes(), 2);
        let mut slices = [&[][..]; PORT_TX_QUEUE];
        let frames = subframes.frames(&mut slices, 5);
        assert_eq!(frames.subframes.len(), 2);
        assert_eq!(frames.subframes[1], &[60; 60][..]);
        assert_eq!(frames.key, key);
        assert_eq!(frames.min_mpdu_start_spacing, 5);
    }

    #[test]
    fn a_full_aggregate_refuses_before_encoding_and_clear_empties_it() {
        let mut subframes = AmpduSubframes::new();
        for _ in 0..PORT_TX_QUEUE {
            assert_eq!(
                subframes.push(|_| Ok::<_, ()>((24, KeySelector::Plaintext))),
                Ok(true)
            );
        }
        assert_eq!(
            subframes.push(|_| -> Result<_, ()> { panic!("encoded past the end") }),
            Ok(false)
        );
        // Plaintext subframes carry no MIC.
        let request = subframes
            .request(0, SequenceNumber::ZERO, Ieee80211Instant::from_micros(0))
            .unwrap();
        assert_eq!(request.mpdu_length(0), 28);
        subframes.clear();
        assert!(subframes.is_empty());
        assert!(
            subframes
                .request(0, SequenceNumber::ZERO, Ieee80211Instant::from_micros(0))
                .is_none()
        );
        assert_eq!(
            subframes.push(|_| Err::<(usize, KeySelector), _>(7)),
            Err(7)
        );
        assert!(subframes.is_empty());
    }
}
