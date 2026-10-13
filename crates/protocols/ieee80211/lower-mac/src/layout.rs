//! Checked placement of a MAC header, optional CCMP header and external body.

use crate::owner::FrameHeader;
use crate::{
    ClassSubframe, Ieee80211Capabilities, IndividualAddress, PreparationError, Tid, TxBufferClass,
    TxFrame,
};

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

/// An immutable proof of the MAC/CCMP prefix and tail-body placement.
/// Constructors use a checked frame length. Backends keep this proof beside
/// its buffer through preparation, admission and return; they do not repeat
/// portable layout checks or allocate a PN before this proof exists.
#[derive(Clone, Copy, Debug)]
pub struct MpduLayout {
    len: usize,
    header_len: usize,
    prefix_len: usize,
    body_start: usize,
    protected: bool,
    kind: MpduKind,
    recipient: Option<(IndividualAddress, Tid)>,
}
impl MpduLayout {
    /// Check the only dynamic layout constraints: CCMP space and body overlap.
    pub fn new<K: Ieee80211Capabilities>(
        frame: &TxFrame<'_, K>,
        body_len: usize,
        protected: bool,
    ) -> Result<Self, PreparationError> {
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
            protected,
            kind,
            recipient,
        )
    }
    /// Check a fresh subframe using the builder's protection and recipient.
    pub fn subframe<C: TxBufferClass, K: Ieee80211Capabilities>(
        frame: &ClassSubframe<C, K>,
        body_len: usize,
        protected: bool,
        receiver: IndividualAddress,
        tid: Tid,
    ) -> Result<Self, PreparationError> {
        Self::checked(
            frame.frame().len().get(),
            frame.frame().header().header_len(),
            body_len,
            protected,
            MpduKind::Individual,
            Some((receiver, tid)),
        )
    }
    fn checked(
        len: usize,
        header_len: usize,
        body_len: usize,
        protected: bool,
        kind: MpduKind,
        recipient: Option<(IndividualAddress, Tid)>,
    ) -> Result<Self, PreparationError> {
        let prefix_len = if protected {
            header_len
                .checked_add(8)
                .filter(|end| *end <= len)
                .ok_or(PreparationError::CcmpHeaderTooShort)?
        } else {
            header_len
        };
        let body_start = len
            .checked_sub(body_len)
            .filter(|start| *start >= prefix_len)
            .ok_or(PreparationError::BodyOverlap)?;
        Ok(Self {
            len,
            header_len,
            prefix_len,
            body_start,
            protected,
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
    /// Whether the builder/preparation selected CCMP.
    pub const fn protected(self) -> bool {
        self.protected
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
