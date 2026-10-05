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
    AmpduFrames, MpduParts,
    client::{PortClient, PortClientEnv, PortClientError, PortError},
    frame::{PORT_MPDU_HEADER_CAPACITY, split_ethernet},
};
use oer_ieee80211_datapath::SoftwareTxFrame;

const FCS_LEN: u16 = 4;

/// Subframes of one A-MPDU a service builds.
pub const PORT_AMPDU_SUBFRAMES: usize = 32;
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

/// The subframes of one A-MPDU, at most [`PORT_AMPDU_SUBFRAMES`]: each the
/// header a service encodes from a network frame's Ethernet header and that
/// frame's payload, borrowed from its owner, which the subframes keep until
/// the next [`Self::clear`]. Every subframe is under the key of the last
/// one pushed.
pub struct AmpduSubframes<F> {
    headers: [[u8; PORT_MPDU_HEADER_CAPACITY]; PORT_AMPDU_SUBFRAMES],
    lengths: [usize; PORT_AMPDU_SUBFRAMES],
    owners: [Option<F>; PORT_AMPDU_SUBFRAMES],
    count: usize,
    key: KeySelector,
}

impl<F: SoftwareTxFrame> Default for AmpduSubframes<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F: SoftwareTxFrame> AmpduSubframes<F> {
    pub const fn new() -> Self {
        Self {
            headers: [[0; PORT_MPDU_HEADER_CAPACITY]; PORT_AMPDU_SUBFRAMES],
            lengths: [0; PORT_AMPDU_SUBFRAMES],
            owners: [const { None }; PORT_AMPDU_SUBFRAMES],
            count: 0,
            key: KeySelector::Plaintext,
        }
    }

    /// Begin another aggregate: the owners of the last one go back to the
    /// network.
    pub fn clear(&mut self) {
        for owner in &mut self.owners[..self.count] {
            *owner = None;
        }
        self.count = 0;
        self.key = KeySelector::Plaintext;
    }

    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub const fn is_full(&self) -> bool {
        self.count == PORT_AMPDU_SUBFRAMES
    }

    /// Add `frame` as the next subframe under `key`: `header`, which the
    /// service encoded from the frame's Ethernet header ([`split_ethernet`]),
    /// and the frame's payload. The frame back when every subframe is taken
    /// or the header exceeds [`PORT_MPDU_HEADER_CAPACITY`].
    pub fn push(&mut self, frame: F, header: &[u8], key: KeySelector) -> Result<(), F> {
        let Some(slot) = self.headers.get_mut(self.count) else {
            return Err(frame);
        };
        let Some(slot) = slot.get_mut(..header.len()) else {
            return Err(frame);
        };
        slot.copy_from_slice(header);
        self.lengths[self.count] = header.len();
        self.owners[self.count] = Some(frame);
        self.key = key;
        self.count += 1;
        Ok(())
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
        let mut on_air = [0_u16; PORT_AMPDU_SUBFRAMES];
        for (index, on_air) in on_air.iter_mut().enumerate().take(self.count) {
            *on_air = self.parts(index).len() as u16 + trailer;
        }
        AmpduRequest::new(
            tid,
            first_sequence,
            &on_air[..self.count],
            committed_at,
            true,
        )
    }

    /// The frames the client sends, their parts in `parts`, to a recipient
    /// of `min_mpdu_start_spacing` (IEEE encoding 0-7).
    pub fn frames<'a>(
        &'a self,
        parts: &'a mut [MpduParts<'a>; PORT_AMPDU_SUBFRAMES],
        min_mpdu_start_spacing: u8,
    ) -> AmpduFrames<'a> {
        for (index, part) in parts.iter_mut().enumerate().take(self.count) {
            *part = self.parts(index);
        }
        AmpduFrames {
            subframes: &parts[..self.count],
            key: self.key,
            min_mpdu_start_spacing,
        }
    }

    fn parts(&self, index: usize) -> MpduParts<'_> {
        let body = self.owners[index]
            .as_ref()
            .map_or(&[][..], |owner| split_ethernet(owner.ethernet()).1);
        MpduParts::new(&self.headers[index][..self.lengths[index]], body)
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use std::{rc::Rc, vec, vec::Vec};

    use oer_ieee80211_lower_mac::KeyHandle;
    use oer_ieee80211_mac::data::ETHERNET_HEADER_LEN;
    use oer_network_interface::NetworkInterfaceId;

    use super::*;

    /// A network frame that counts its return.
    struct Frame(Vec<u8>, Rc<Cell<usize>>);

    impl SoftwareTxFrame for Frame {
        fn interface(&self) -> NetworkInterfaceId {
            NetworkInterfaceId::new(0)
        }

        fn ethernet(&self) -> &[u8] {
            &self.0
        }
    }

    impl Drop for Frame {
        fn drop(&mut self) {
            self.1.set(self.1.get() + 1);
        }
    }

    fn frame(payload: &[u8], returned: &Rc<Cell<usize>>) -> Frame {
        let mut ethernet = vec![0x02; ETHERNET_HEADER_LEN];
        ethernet.extend_from_slice(payload);
        Frame(ethernet, returned.clone())
    }

    #[test]
    fn a_subframe_is_its_encoded_header_and_its_owner_s_payload() {
        let returned = Rc::new(Cell::new(0));
        let mut subframes = AmpduSubframes::new();
        let key = KeySelector::Key(KeyHandle(3));
        for (length, payload) in [(40, &b"first"[..]), (42, b"second")] {
            let header = [length as u8; 64];
            assert!(
                subframes
                    .push(frame(payload, &returned), &header[..length], key)
                    .is_ok()
            );
        }
        let request = subframes
            .request(0, SequenceNumber::ZERO, Ieee80211Instant::from_micros(0))
            .unwrap();
        // Header and payload, FCS and MIC on air.
        assert_eq!(
            (request.mpdu_length(0), request.mpdu_length(1)),
            (40 + 5 + 12, 42 + 6 + 12)
        );
        let mut parts = [MpduParts::default(); PORT_AMPDU_SUBFRAMES];
        let frames = subframes.frames(&mut parts, 5);
        assert_eq!(frames.subframes.len(), 2);
        assert_eq!(frames.subframes[1].header, &[42; 42][..]);
        assert_eq!(frames.subframes[1].body, b"second");
        assert_eq!((frames.key, frames.min_mpdu_start_spacing), (key, 5));
        // The owners stay until the aggregate ends.
        assert_eq!(returned.get(), 0);
        subframes.clear();
        assert_eq!(returned.get(), 2);
        assert!(subframes.is_empty());
    }

    #[test]
    fn a_full_aggregate_gives_the_frame_back_before_encoding() {
        let returned = Rc::new(Cell::new(0));
        let mut subframes = AmpduSubframes::new();
        for _ in 0..PORT_AMPDU_SUBFRAMES {
            assert!(
                subframes
                    .push(frame(b"x", &returned), &[0; 24], KeySelector::Plaintext)
                    .is_ok()
            );
        }
        assert!(subframes.is_full());
        let Err(late) = subframes.push(frame(b"late", &returned), &[0; 24], KeySelector::Plaintext)
        else {
            panic!("the frame was not given back");
        };
        assert_eq!(&late.0[ETHERNET_HEADER_LEN..], b"late");
        assert_eq!(returned.get(), 0);
        // Plaintext subframes carry no MIC.
        let request = subframes
            .request(0, SequenceNumber::ZERO, Ieee80211Instant::from_micros(0))
            .unwrap();
        assert_eq!(request.mpdu_length(0), 24 + 1 + 4);
        assert_eq!(request.subframes() as usize, PORT_AMPDU_SUBFRAMES);
    }

    #[test]
    fn a_header_beyond_its_capacity_gives_the_frame_back() {
        let returned = Rc::new(Cell::new(0));
        let mut subframes = AmpduSubframes::new();
        let header = [0; PORT_MPDU_HEADER_CAPACITY + 1];
        assert!(
            subframes
                .push(frame(b"x", &returned), &header, KeySelector::Plaintext)
                .is_err()
        );
        assert!(subframes.is_empty());
        assert!(
            subframes
                .request(0, SequenceNumber::ZERO, Ieee80211Instant::from_micros(0))
                .is_none()
        );
    }
}
