//! The CCMP replay check of one association's keys, shared by the station
//! and the access point: the receive frontiers of the pairwise key and of
//! the group key, each with one lane for non-QoS frames and one per TID
//! ([`CcmpRxReplayState`]); a new group key starts the group's frontiers
//! from its receive sequence.
//!
//! The receive path checks a frame here once it is decrypted and verified,
//! and after the reorder of its TID, when the receiver has one: within a
//! lane the packet numbers then strictly increase. Moved from the ESP32-S31
//! station's connected receive path ([layering decision on #563]).
//!
//! [layering decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225

use super::{CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane, CcmpRxReplayCandidate, CcmpRxReplayState};

/// Which key a received frame's destination selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpRxProtection {
    /// An individually addressed frame: the pairwise key.
    Pairwise,
    /// A group addressed frame: the group key.
    Group,
    /// A frame for neither.
    Other,
}

/// Why the replay check refused a frame or a key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpRxReplayError {
    /// The pairwise key's receive sequence is not a packet number.
    InvalidPairwiseReceiveSequence,
    /// The group key's receive sequence is not a packet number.
    InvalidGroupReceiveSequence,
    /// The group key's index is not 0 to 3.
    InvalidGroupKeyId(u8),
    /// A group key change prepared against another epoch.
    StaleGroupRotation,
    /// The group key changed too many times.
    GroupRevisionExhausted,
    /// The frame is for neither key.
    ForeignDestination,
    /// The frame's CCMP header names another key than its destination's.
    UnexpectedKeyId {
        /// The destination's key.
        protection: CcmpRxProtection,
        /// The key index it expects.
        expected: CcmpKeyId,
        /// The key index the header names.
        observed: CcmpKeyId,
    },
    /// The frame repeats or precedes a packet number of its lane.
    Replay(CcmpReplayError),
}

/// A prepared check of one frame; it commits once the frame is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpRxReplayEpochCandidate {
    /// A pairwise frame's.
    Pairwise(CcmpRxReplayCandidate),
    /// A group frame's.
    Group(CcmpRxReplayCandidate),
}

/// The receive frontiers of one association's pairwise and group keys.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpRxReplayEpoch {
    pairwise: CcmpRxReplayState,
    group: CcmpRxReplayState,
    group_key_id: CcmpKeyId,
    group_revision: u32,
}

/// A group key change prepared against one epoch.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpGroupReplayReplacement {
    expected_key_id: CcmpKeyId,
    expected_revision: u32,
    group: CcmpRxReplayState,
    group_key_id: CcmpKeyId,
}

impl CcmpRxReplayEpoch {
    /// The frontiers of a new pairwise key and of the group key
    /// `group_key_id`, from their receive sequences.
    ///
    /// # Errors
    ///
    /// A receive sequence that is no packet number, or a key index above 3.
    pub fn new(
        pairwise_receive_sequence: [u8; 8],
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<Self, CcmpRxReplayError> {
        let pairwise = CcmpRxReplayState::from_receive_sequence(pairwise_receive_sequence)
            .map_err(|_| CcmpRxReplayError::InvalidPairwiseReceiveSequence)?;
        let group = CcmpRxReplayState::from_receive_sequence(group_receive_sequence)
            .map_err(|_| CcmpRxReplayError::InvalidGroupReceiveSequence)?;
        let group_key_id = CcmpKeyId::new(group_key_id)
            .ok_or(CcmpRxReplayError::InvalidGroupKeyId(group_key_id))?;
        Ok(Self {
            pairwise,
            group,
            group_key_id,
            group_revision: 0,
        })
    }

    /// The group key's index.
    pub const fn group_key_id(&self) -> CcmpKeyId {
        self.group_key_id
    }

    /// Prepare the check of a frame of `protection`, of the TID `tid` (none
    /// for a non-QoS frame), with the CCMP header `header`.
    ///
    /// # Errors
    ///
    /// A foreign destination, a key index that does not match it, or a
    /// replayed packet number.
    pub fn prepare(
        &self,
        protection: CcmpRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<CcmpRxReplayEpochCandidate, CcmpRxReplayError> {
        let lane = match tid {
            Some(tid) => CcmpReplayLane::Tid(tid),
            None => CcmpReplayLane::NonQos,
        };
        match protection {
            CcmpRxProtection::Pairwise => {
                if header.key_id() != CcmpKeyId::PAIRWISE {
                    return Err(CcmpRxReplayError::UnexpectedKeyId {
                        protection,
                        expected: CcmpKeyId::PAIRWISE,
                        observed: header.key_id(),
                    });
                }
                self.pairwise
                    .prepare(lane, header.packet_number())
                    .map(CcmpRxReplayEpochCandidate::Pairwise)
                    .map_err(CcmpRxReplayError::Replay)
            }
            CcmpRxProtection::Group => {
                if header.key_id() != self.group_key_id {
                    return Err(CcmpRxReplayError::UnexpectedKeyId {
                        protection,
                        expected: self.group_key_id,
                        observed: header.key_id(),
                    });
                }
                self.group
                    .prepare(lane, header.packet_number())
                    .map(CcmpRxReplayEpochCandidate::Group)
                    .map_err(CcmpRxReplayError::Replay)
            }
            CcmpRxProtection::Other => Err(CcmpRxReplayError::ForeignDestination),
        }
    }

    /// Commit a prepared check: its lane's frontier advances.
    ///
    /// # Errors
    ///
    /// A candidate another commit made stale.
    pub fn commit(&mut self, candidate: CcmpRxReplayEpochCandidate) -> Result<(), CcmpRxReplayError> {
        match candidate {
            CcmpRxReplayEpochCandidate::Pairwise(candidate) => self
                .pairwise
                .commit(candidate)
                .map_err(CcmpRxReplayError::Replay),
            CcmpRxReplayEpochCandidate::Group(candidate) => self
                .group
                .commit(candidate)
                .map_err(CcmpRxReplayError::Replay),
        }
    }

    /// Check a frame that is published at once: prepare and commit.
    ///
    /// # Errors
    ///
    /// As [`Self::prepare`].
    pub fn check(
        &mut self,
        protection: CcmpRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<(), CcmpRxReplayError> {
        let candidate = self.prepare(protection, tid, header)?;
        self.commit(candidate)
    }

    /// Raise `group`'s frontiers to this epoch's where this epoch's are
    /// higher: a group key installed again under the same index never
    /// lowers them.
    fn merge_group_frontiers(&self, group: &mut CcmpRxReplayState) -> Result<(), CcmpRxReplayError> {
        for lane in core::iter::once(CcmpReplayLane::NonQos).chain((0..16).map(CcmpReplayLane::Tid))
        {
            let current = self
                .group
                .highest(lane)
                .ok_or(CcmpRxReplayError::Replay(CcmpReplayError::InvalidTid))?;
            let incoming = group
                .highest(lane)
                .ok_or(CcmpRxReplayError::Replay(CcmpReplayError::InvalidTid))?;
            if current > incoming {
                let candidate = group
                    .prepare(lane, current)
                    .map_err(CcmpRxReplayError::Replay)?;
                group.commit(candidate).map_err(CcmpRxReplayError::Replay)?;
            }
        }
        Ok(())
    }

    /// Prepare the change to the group key `group_key_id` with its receive
    /// sequence: a new key starts its frontiers there, the same key keeps
    /// the higher of its old and new ones.
    ///
    /// # Errors
    ///
    /// A receive sequence that is no packet number, or a key index above 3.
    pub fn prepare_group_rotation(
        &self,
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<CcmpGroupReplayReplacement, CcmpRxReplayError> {
        let mut group = CcmpRxReplayState::from_receive_sequence(group_receive_sequence)
            .map_err(|_| CcmpRxReplayError::InvalidGroupReceiveSequence)?;
        let group_key_id = CcmpKeyId::new(group_key_id)
            .ok_or(CcmpRxReplayError::InvalidGroupKeyId(group_key_id))?;
        if group_key_id == self.group_key_id {
            self.merge_group_frontiers(&mut group)?;
        }
        Ok(CcmpGroupReplayReplacement {
            expected_key_id: self.group_key_id,
            expected_revision: self.group_revision,
            group,
            group_key_id,
        })
    }

    /// Commit a prepared group key change, once the new key is installed.
    ///
    /// # Errors
    ///
    /// A change prepared against another epoch, or too many changes.
    pub fn commit_group_rotation(
        &mut self,
        replacement: CcmpGroupReplayReplacement,
    ) -> Result<(), CcmpRxReplayError> {
        if self.group_key_id != replacement.expected_key_id
            || self.group_revision != replacement.expected_revision
        {
            return Err(CcmpRxReplayError::StaleGroupRotation);
        }
        let revision = self
            .group_revision
            .checked_add(1)
            .ok_or(CcmpRxReplayError::GroupRevisionExhausted)?;
        self.group = replacement.group;
        self.group_key_id = replacement.group_key_id;
        self.group_revision = revision;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
