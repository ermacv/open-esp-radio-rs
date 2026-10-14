//! Checked placement of a MAC header, optional CCMP header and external body.

use crate::owner::FrameHeader;
use crate::{ClassSubframe, Ieee80211Capabilities, Tid, TxBufferClass, TxFrame};
use oer_ieee80211_mac::address::IndividualAddress;

/// Portable refusals before the backend receives a prepared source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// The supplied allocation cannot hold the described frame or header.
    StorageTooShort,
    /// No room after the MAC header for a complete CCMP header.
    CcmpHeaderTooShort,
    /// The external body overlaps the MAC/CCMP prefix.
    BodyOverlap,
}

/// A MAC header and its exact-size writable array. Only this constructor
/// checks storage length; the selected encoder takes an array and is total.
///
/// A caller cannot replace the header's acknowledgement policy at encoding:
///
/// ```compile_fail,E0061
/// use oer_ieee80211_lower_mac::HeaderLayout;
/// use oer_ieee80211_mac::address::IndividualAddress;
/// fn policy(layout: HeaderLayout<'_, IndividualAddress>) { layout.encode(true); }
/// ```
///
/// An individual header must have a checked response kind:
///
/// ```compile_fail,E0277
/// use oer_ieee80211_lower_mac::{HeaderLayout, HeaderSpec};
/// use oer_ieee80211_mac::address::IndividualAddress;
/// fn raw(header: HeaderSpec<IndividualAddress>, out: &mut [u8]) {
///     drop(HeaderLayout::new(header, out));
/// }
/// ```
///
/// Each accepted header type carries its own response policy:
///
/// ```
/// use oer_ieee80211_lower_mac::{
///     AckHeader, GroupAddress, HeaderLayout, HeaderSpec, LayoutError, NoAckHeader,
/// };
/// fn typed(
///     ack: AckHeader, no_ack: NoAckHeader, group: HeaderSpec<GroupAddress>, out: &mut [u8],
/// ) -> Result<(), LayoutError> {
///     HeaderLayout::new(ack, out)?.encode();
///     HeaderLayout::new(no_ack, out)?.encode();
///     HeaderLayout::new(group, out)?.encode();
///     Ok(())
/// }
/// ```
pub struct HeaderLayout<'a, A> {
    inner: HeaderStorage<'a, A>,
}
enum HeaderStorage<'a, A> {
    H24(crate::owner::HeaderEncoder<A, 24>, &'a mut [u8; 24]),
    H26(crate::owner::HeaderEncoder<A, 26>, &'a mut [u8; 26]),
    H28(crate::owner::HeaderEncoder<A, 28>, &'a mut [u8; 28]),
    H30(crate::owner::HeaderEncoder<A, 30>, &'a mut [u8; 30]),
    H32(crate::owner::HeaderEncoder<A, 32>, &'a mut [u8; 32]),
    H34(crate::owner::HeaderEncoder<A, 34>, &'a mut [u8; 34]),
    H36(crate::owner::HeaderEncoder<A, 36>, &'a mut [u8; 36]),
}
impl<'a, A: crate::ReceiverAddress> HeaderLayout<'a, A> {
    /// Prove the storage length, without writing any bytes on refusal.
    pub fn new<H: crate::ResponseHeader<Address = A>>(
        header: H,
        out: &'a mut [u8],
    ) -> Result<Self, LayoutError> {
        use crate::owner::HeaderLength;
        let policy = H::ACK_POLICY;
        let header = header.into_header();
        let inner = match header.encoded_length() {
            HeaderLength::L24 => HeaderStorage::H24(
                header.encoder(policy),
                out.split_first_chunk_mut::<24>()
                    .ok_or(LayoutError::StorageTooShort)?
                    .0,
            ),
            HeaderLength::L26 => HeaderStorage::H26(
                header.encoder(policy),
                out.split_first_chunk_mut::<26>()
                    .ok_or(LayoutError::StorageTooShort)?
                    .0,
            ),
            HeaderLength::L28 => HeaderStorage::H28(
                header.encoder(policy),
                out.split_first_chunk_mut::<28>()
                    .ok_or(LayoutError::StorageTooShort)?
                    .0,
            ),
            HeaderLength::L30 => HeaderStorage::H30(
                header.encoder(policy),
                out.split_first_chunk_mut::<30>()
                    .ok_or(LayoutError::StorageTooShort)?
                    .0,
            ),
            HeaderLength::L32 => HeaderStorage::H32(
                header.encoder(policy),
                out.split_first_chunk_mut::<32>()
                    .ok_or(LayoutError::StorageTooShort)?
                    .0,
            ),
            HeaderLength::L34 => HeaderStorage::H34(
                header.encoder(policy),
                out.split_first_chunk_mut::<34>()
                    .ok_or(LayoutError::StorageTooShort)?
                    .0,
            ),
            HeaderLength::L36 => HeaderStorage::H36(
                header.encoder(policy),
                out.split_first_chunk_mut::<36>()
                    .ok_or(LayoutError::StorageTooShort)?
                    .0,
            ),
        };
        Ok(Self { inner })
    }
    /// Encode into the array whose length this layout established.
    pub fn encode(self) {
        match self.inner {
            HeaderStorage::H24(header, out) => header.encode(out),
            HeaderStorage::H26(header, out) => header.encode(out),
            HeaderStorage::H28(header, out) => header.encode(out),
            HeaderStorage::H30(header, out) => header.encode(out),
            HeaderStorage::H32(header, out) => header.encode(out),
            HeaderStorage::H34(header, out) => header.encode(out),
            HeaderStorage::H36(header, out) => header.encode(out),
        }
    }
}

/// The response kind retained by a published MPDU, independent of a new plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpduKind {
    /// Individually acknowledged data or management frame.
    Individual,
    /// Individually addressed without an acknowledgement.
    IndividualNoAck,
    /// Group-addressed frame.
    Group,
    /// Block Ack request.
    BlockAckRequest,
}

/// The MPDU encryption selected by its operation or aggregate builder.
/// This layout value carries no key; the preparation retains the installed
/// key separately in its `TxProtection` and derives this value from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpduProtection {
    /// An unencrypted MAC frame, with no CCMP prefix.
    Open,
    /// A CCMP frame with its reserved CCMP header.
    Ccmp,
}

/// An immutable proof of the MAC/CCMP prefix and tail-body placement.
/// Constructors use a checked frame length. Backends keep this proof beside
/// its buffer through preparation, admission and return; they do not repeat
/// portable layout checks or allocate a PN before this proof exists.
///
/// ```compile_fail,E0308
/// use oer_ieee80211_lower_mac::{Ieee80211Capabilities, MpduLayout, TxFrame};
/// fn flag<K: Ieee80211Capabilities>(frame: &TxFrame<'_, K>) {
///     drop(MpduLayout::new(frame, 0, true));
/// }
/// ```
///
/// ```
/// use oer_ieee80211_lower_mac::{
///     Ieee80211Capabilities, LayoutError, MpduLayout, MpduProtection, TxFrame,
/// };
/// fn open<K: Ieee80211Capabilities>(frame: &TxFrame<'_, K>) -> Result<(), LayoutError> {
///     let layout = MpduLayout::new(frame, 0, MpduProtection::Open)?;
///     assert_eq!(layout.protection(), MpduProtection::Open);
///     Ok(())
/// }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct MpduLayout {
    len: usize,
    header_len: usize,
    prefix_len: usize,
    body_start: usize,
    protection: MpduProtection,
    kind: MpduKind,
    recipient: Option<(IndividualAddress, Tid)>,
}
impl MpduLayout {
    /// Check the only dynamic layout constraints: CCMP space and body overlap.
    pub fn new<K: Ieee80211Capabilities>(
        frame: &TxFrame<'_, K>,
        body_len: usize,
        protection: MpduProtection,
    ) -> Result<Self, LayoutError> {
        let (header_len, kind, recipient) = match frame {
            TxFrame::Individual { frame, .. } => {
                let header = frame.header().header();
                (
                    header.len(),
                    MpduKind::Individual,
                    header.qos().map(|qos| (header.receiver(), qos.tid)),
                )
            }
            TxFrame::IndividualNoAck { frame, .. } => {
                (frame.header().header_len(), MpduKind::IndividualNoAck, None)
            }
            TxFrame::Group { frame, .. } => (frame.header().header_len(), MpduKind::Group, None),
            TxFrame::BlockAckRequest { .. } => (20, MpduKind::BlockAckRequest, None),
        };
        Self::checked(
            frame.mpdu_len(),
            header_len,
            body_len,
            protection,
            kind,
            recipient,
        )
    }
    /// Check a fresh subframe using the builder's protection and recipient.
    pub fn subframe<C: TxBufferClass, K: Ieee80211Capabilities>(
        frame: &ClassSubframe<C, K>,
        body_len: usize,
        protection: MpduProtection,
        receiver: IndividualAddress,
        tid: Tid,
    ) -> Result<Self, LayoutError> {
        Self::checked(
            frame.frame().len().get(),
            frame.frame().header().header_len(),
            body_len,
            protection,
            MpduKind::Individual,
            Some((receiver, tid)),
        )
    }
    fn checked(
        len: usize,
        header_len: usize,
        body_len: usize,
        protection: MpduProtection,
        kind: MpduKind,
        recipient: Option<(IndividualAddress, Tid)>,
    ) -> Result<Self, LayoutError> {
        let prefix_len = match protection {
            MpduProtection::Open => header_len,
            MpduProtection::Ccmp => header_len
                .checked_add(oer_ieee80211_mac::ccmp::CCMP_HEADER_LEN)
                .filter(|end| *end <= len)
                .ok_or(LayoutError::CcmpHeaderTooShort)?,
        };
        let body_start = len
            .checked_sub(body_len)
            .filter(|start| *start >= prefix_len)
            .ok_or(LayoutError::BodyOverlap)?;
        Ok(Self {
            len,
            header_len,
            prefix_len,
            body_start,
            protection,
            kind,
            recipient,
        })
    }
    /// Whole MPDU length, excluding FCS and hardware MIC.
    pub const fn len(self) -> usize {
        self.len
    }
    /// MAC headers are never empty.
    pub const fn is_empty(self) -> bool {
        false
    }
    /// MAC header length.
    pub const fn header_len(self) -> usize {
        self.header_len
    }
    /// End of the reserved MAC/CCMP prefix.
    pub const fn prefix_len(self) -> usize {
        self.prefix_len
    }
    /// First external-body octet, or end of MPDU for no external body.
    pub const fn body_start(self) -> usize {
        self.body_start
    }
    /// The encryption selected by the builder or preparation.
    pub const fn protection(self) -> MpduProtection {
        self.protection
    }
    /// Response kind retained for a retransmission.
    pub const fn kind(self) -> MpduKind {
        self.kind
    }
    /// QoS recipient retained for aggregate retransmission admission.
    pub const fn recipient(self) -> Option<(IndividualAddress, Tid)> {
        self.recipient
    }
}

/// A refusal from a portable layout constructor returns its original owners.
pub struct LayoutRefused<T> {
    /// The failed layout constraint.
    pub reason: LayoutError,
    /// Original source, or builder and subframe, before admission and PN.
    pub operation: T,
}

/// Fresh owners kept with the immutable layout established for exactly them.
pub struct LaidOut<T> {
    source: T,
    layout: MpduLayout,
}
impl<T> LaidOut<T> {
    /// Transfer the owners and their established layout to the backend.
    pub fn into_parts(self) -> (T, MpduLayout) {
        (self.source, self.layout)
    }
    /// Cancel preparation without fabricating a TX terminal.
    pub fn into_source(self) -> T {
        self.source
    }
}

/// The original fresh MPDU owners, before or after checking their layout.
pub struct FreshSource<B, F, O, Key> {
    /// Fresh memory.
    pub buffer: B,
    /// Class, phase and capabilities proof.
    pub frame: F,
    /// External body, if any.
    pub body: Option<O>,
    /// The sole source of encryption and the Protected bit.
    pub protection: crate::TxProtection<Key>,
}

/// Input to backend preparation. Fresh layout has already been checked;
/// retransmission carries the returned, immutable MPDU itself.
pub enum CheckedTxSource<'p, B, N, F, O, Key> {
    /// Fresh memory and its portable layout proof.
    Fresh(LaidOut<FreshSource<B, F, O, Key>>),
    /// The original published MPDU and only the new attempt's plan.
    Retransmission {
        mpdu: N,
        plan: crate::RetransmissionPlan<'p>,
    },
}
impl<'p, B, N, F, O, Key> CheckedTxSource<'p, B, N, F, O, Key> {
    /// Recover the complete source on cancellation or backend refusal.
    pub fn into_source(self) -> crate::TxSource<'p, B, N, F, O, Key> {
        match self {
            Self::Fresh(fresh) => {
                let FreshSource {
                    buffer,
                    frame,
                    body,
                    protection,
                } = fresh.into_source();
                crate::TxSource::Fresh {
                    buffer,
                    frame,
                    body,
                    protection,
                }
            }
            Self::Retransmission { mpdu, plan } => crate::TxSource::Retransmission { mpdu, plan },
        }
    }
}

impl<
    'p,
    B: crate::TxBufferMemory,
    N,
    C: TxBufferClass,
    K: Ieee80211Capabilities,
    O: crate::TxBody,
    Key,
> crate::TxSource<'p, B, N, crate::ClassFrame<'p, C, K>, O, Key>
{
    /// Prove the complete layout before entering a backend. A refusal does
    /// not change memory, consume PN, take admission or create a terminal.
    pub fn check(self) -> SourceLayoutResult<'p, B, N, crate::ClassFrame<'p, C, K>, O, Key> {
        match self {
            Self::Retransmission { mpdu, plan } => {
                Ok(CheckedTxSource::Retransmission { mpdu, plan })
            }
            Self::Fresh {
                mut buffer,
                frame,
                body,
                protection,
            } => {
                let checked = MpduLayout::new(
                    frame.frame(),
                    body.as_ref().map_or(0, |body| body.bytes().len()),
                    protection.layout(),
                )
                .and_then(|layout| {
                    layout.initialize(
                        buffer.bytes_mut(),
                        frame.frame(),
                        body.as_ref().map(crate::TxBody::bytes),
                    )
                });
                match checked {
                    Ok(layout) => Ok(CheckedTxSource::Fresh(LaidOut {
                        source: FreshSource {
                            buffer,
                            frame,
                            body,
                            protection,
                        },
                        layout,
                    })),
                    Err(reason) => Err(LayoutRefused {
                        reason,
                        operation: Self::Fresh {
                            buffer,
                            frame,
                            body,
                            protection,
                        },
                    }),
                }
            }
        }
    }
}

impl MpduLayout {
    fn memory(self, bytes: &mut [u8]) -> Result<(&mut [u8], &mut [u8]), LayoutError> {
        let (mpdu, _) = bytes
            .split_at_mut_checked(self.len)
            .ok_or(LayoutError::StorageTooShort)?;
        mpdu.split_at_mut_checked(self.body_start)
            .ok_or(LayoutError::StorageTooShort)
    }
    fn initialize<K: Ieee80211Capabilities>(
        self,
        bytes: &mut [u8],
        frame: &TxFrame<'_, K>,
        body: Option<&[u8]>,
    ) -> Result<Self, LayoutError> {
        let (prefix, body_memory) = self.memory(bytes)?;
        match frame {
            TxFrame::Individual { frame, .. } => {
                HeaderLayout::new(frame.header().with_protection(self.protection), prefix)?.encode()
            }
            TxFrame::IndividualNoAck { frame, .. } => {
                HeaderLayout::new(frame.header().with_protection(self.protection), prefix)?.encode()
            }
            TxFrame::Group { frame, .. } => {
                HeaderLayout::new(frame.header().with_protection(self.protection), prefix)?.encode()
            }
            TxFrame::BlockAckRequest {
                receiver,
                transmitter,
                tid,
                starting_sequence,
                ..
            } => {
                let (header, _) = prefix
                    .split_first_chunk_mut::<20>()
                    .ok_or(LayoutError::StorageTooShort)?;
                *header = oer_ieee80211_mac::block_ack::encode_block_ack_request(
                    receiver.get(),
                    *transmitter,
                    tid.get(),
                    *starting_sequence,
                );
            }
        }
        // checked() and memory() prove exactly body.len() tail bytes.
        if let Some(body) = body {
            for (out, byte) in body_memory.iter_mut().zip(body) {
                *out = *byte;
            }
        }
        Ok(self)
    }
    fn initialize_subframe<C: TxBufferClass, K: Ieee80211Capabilities>(
        self,
        bytes: &mut [u8],
        frame: &ClassSubframe<C, K>,
        body: Option<&[u8]>,
        receiver: IndividualAddress,
        tid: Tid,
    ) -> Result<Self, LayoutError> {
        let (prefix, body_memory) = self.memory(bytes)?;
        HeaderLayout::new(frame.header(receiver, tid, self.protection), prefix)?.encode();
        if let Some(body) = body {
            for (out, byte) in body_memory.iter_mut().zip(body) {
                *out = *byte;
            }
        }
        Ok(self)
    }
}

/// Fresh aggregate owners have no independent protection.
pub struct FreshSubframe<B, F, O> {
    /// Fresh memory.
    pub buffer: B,
    /// The subframe's checked description.
    pub frame: F,
    /// Its external body.
    pub body: Option<O>,
}
/// Checked fresh placement, or the original Sent MPDU.
pub enum CheckedSubframe<B, N, F, O> {
    /// Protection, recipient and TID were taken from the owned builder.
    Fresh(LaidOut<FreshSubframe<B, F, O>>),
    /// The original published MPDU.
    Sent(N),
}
impl<B, N, F, O> CheckedSubframe<B, N, F, O> {
    /// Recover every subframe owner.
    pub fn into_source(self) -> crate::SubframeSource<B, N, F, O> {
        match self {
            Self::Fresh(fresh) => {
                let FreshSubframe {
                    buffer,
                    frame,
                    body,
                } = fresh.into_source();
                crate::SubframeSource::Fresh {
                    buffer,
                    frame,
                    body,
                }
            }
            Self::Sent(mpdu) => crate::SubframeSource::Sent(mpdu),
        }
    }
}
/// A builder and a subframe checked together. The private fields prevent
/// moving a fresh layout proof to a differently protected builder.
pub struct CheckedAggregate<A, B, N, F, O> {
    aggregate: A,
    source: CheckedSubframe<B, N, F, O>,
}
impl<A, B, N, F, O> CheckedAggregate<A, B, N, F, O> {
    /// Transfer the builder and its checked subframe to its backend.
    pub fn into_parts(self) -> (A, CheckedSubframe<B, N, F, O>) {
        (self.aggregate, self.source)
    }
}
impl<A, B: crate::TxBufferMemory, N, C: TxBufferClass, K: Ieee80211Capabilities, O: crate::TxBody>
    CheckedAggregate<A, B, N, ClassSubframe<C, K>, O>
{
    /// Establish a fresh layout using exactly this builder's protection.
    pub fn new<'id>(
        aggregate: A,
        source: crate::SubframeSource<B, N, ClassSubframe<C, K>, O>,
    ) -> AggregateLayoutResult<A, B, N, ClassSubframe<C, K>, O>
    where
        A: crate::PhaseAggregate<'id>,
    {
        let source = match source {
            crate::SubframeSource::Sent(mpdu) => CheckedSubframe::Sent(mpdu),
            crate::SubframeSource::Fresh {
                mut buffer,
                frame,
                body,
            } => {
                let checked = MpduLayout::subframe(
                    &frame,
                    body.as_ref().map_or(0, |body| body.bytes().len()),
                    aggregate.protection(),
                    aggregate.receiver(),
                    aggregate.tid(),
                )
                .and_then(|layout| {
                    layout.initialize_subframe(
                        buffer.bytes_mut(),
                        &frame,
                        body.as_ref().map(crate::TxBody::bytes),
                        aggregate.receiver(),
                        aggregate.tid(),
                    )
                });
                match checked {
                    Ok(layout) => CheckedSubframe::Fresh(LaidOut {
                        source: FreshSubframe {
                            buffer,
                            frame,
                            body,
                        },
                        layout,
                    }),
                    Err(reason) => {
                        return Err(LayoutRefused {
                            reason,
                            operation: (
                                aggregate,
                                crate::SubframeSource::Fresh {
                                    buffer,
                                    frame,
                                    body,
                                },
                            ),
                        });
                    }
                }
            }
        };
        Ok(Self { aggregate, source })
    }
}

/// Result of checking an owned single source.
pub type SourceLayoutResult<'p, B, N, F, O, Key> = Result<
    CheckedTxSource<'p, B, N, F, O, Key>,
    LayoutRefused<crate::TxSource<'p, B, N, F, O, Key>>,
>;
/// Result of checking a builder and its subframe together.
pub type AggregateLayoutResult<A, B, N, F, O> =
    Result<CheckedAggregate<A, B, N, F, O>, LayoutRefused<(A, crate::SubframeSource<B, N, F, O>)>>;
