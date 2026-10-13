//! Connection epochs of one installation. The backend mints one source
//! slot for its station; moving that slot into the source consumes it.
//! Dropping/recreating a station cannot mint another source in that install.

use core::marker::PhantomData;
use oer_radio_port::Slot;

/// The single source slot granted to a station by its install.
pub enum LinkEpochKind {}

/// An exhausted source cannot begin another connection in this install.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpochExhausted;

/// A connection identity, invariant in the installation's brand. Only the
/// station's source issues one; neither callers nor Data reconstruct it.
///
/// ```compile_fail
/// use oer_ieee80211_sta::epoch::LinkEpoch;
/// fn cross_install<'old, 'new>(old: LinkEpoch<'old>) -> LinkEpoch<'new> { old }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkEpoch<'id> {
    sequence: u64,
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
}

/// The station's sole, non-cloneable source. A new source consumes the
/// install's unique slot. It has no reset and never gives that slot back;
/// reset requires ending the install, which also ends its branded contexts.
pub struct LinkEpochSource<'id> {
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
    last: u64,
}

impl<'id> LinkEpochSource<'id> {
    /// Consume the one station source slot the backend minted at install.
    pub fn new(_slot: Slot<'id, LinkEpochKind>) -> Self {
        Self {
            brand: PhantomData,
            last: 0,
        }
    }

    pub(crate) fn begin(&mut self) -> Result<LinkEpoch<'id>, EpochExhausted> {
        let next = self.last.checked_add(1).ok_or(EpochExhausted)?;
        self.last = next;
        Ok(LinkEpoch {
            sequence: next,
            brand: PhantomData,
        })
    }
}

#[cfg(test)]
impl<'id> LinkEpoch<'id> {
    pub(crate) const fn for_test(sequence: u64) -> Self {
        Self {
            sequence,
            brand: PhantomData,
        }
    }
}

#[cfg(test)]
impl LinkEpochSource<'_> {
    pub(crate) const fn for_test(last: u64) -> Self {
        Self {
            brand: PhantomData,
            last,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_epoch_is_issued_once_and_exhaustion_never_resets_it() {
        let mut source = LinkEpochSource::for_test(u64::MAX - 1);
        let last = source.begin().unwrap();
        assert_eq!(last.sequence, u64::MAX);
        assert_eq!(source.begin(), Err(EpochExhausted));
        assert_eq!(source.begin(), Err(EpochExhausted));
        assert_eq!(source.last, u64::MAX);
    }
}
