//! Counting the frames of a numbered IEEE 802.15.4 peer stream.

use crate::{IEEE802154_STREAM_CAPACITY, IEEE802154_STREAM_MAGIC, Ieee802154SessionStreamReceipt};

const WORDS: usize = IEEE802154_STREAM_CAPACITY as usize / 32;

/// The counters of one stream a receiver has seen.
#[derive(Clone, Debug)]
pub struct Ieee802154StreamTracker {
    seen: [u32; WORDS],
    duplicates: u16,
    out_of_range: u16,
}

impl Default for Ieee802154StreamTracker {
    fn default() -> Self {
        Self {
            seen: [0; WORDS],
            duplicates: 0,
            out_of_range: 0,
        }
    }
}

/// The stream counter a frame's MAC bytes carry, if it is a stream frame:
/// the first [`IEEE802154_STREAM_MAGIC`] followed by a little-endian counter.
pub fn ieee802154_stream_counter(frame: &[u8]) -> Option<u16> {
    let at = frame
        .windows(IEEE802154_STREAM_MAGIC.len())
        .position(|window| window == IEEE802154_STREAM_MAGIC)?;
    let counter = frame.get(at + IEEE802154_STREAM_MAGIC.len()..)?.get(..2)?;
    Some(u16::from_le_bytes([counter[0], counter[1]]))
}

impl Ieee802154StreamTracker {
    /// Count one received frame; frames without the stream magic are ignored.
    pub fn record(&mut self, frame: &[u8]) {
        let Some(counter) = ieee802154_stream_counter(frame) else {
            return;
        };
        if counter >= IEEE802154_STREAM_CAPACITY {
            self.out_of_range = self.out_of_range.saturating_add(1);
            return;
        }
        let (word, bit) = (usize::from(counter / 32), 1u32 << (counter % 32));
        if self.seen[word] & bit != 0 {
            self.duplicates = self.duplicates.saturating_add(1);
        } else {
            self.seen[word] |= bit;
        }
    }

    /// Summarize the counters seen so far.
    pub fn receipt(&self) -> Ieee802154SessionStreamReceipt {
        let mut receipt = Ieee802154SessionStreamReceipt {
            duplicates: self.duplicates,
            out_of_range: self.out_of_range,
            ..Default::default()
        };
        let mut run = 0u16;
        for counter in 0..IEEE802154_STREAM_CAPACITY {
            let seen = self.seen[usize::from(counter / 32)] & (1 << (counter % 32)) != 0;
            if seen {
                receipt.received += 1;
                receipt.span = counter + 1;
                if run != 0 {
                    receipt.missing_runs += 1;
                    receipt.longest_missing_run = receipt.longest_missing_run.max(run);
                    run = 0;
                }
            } else {
                run += 1;
            }
        }
        receipt
    }
}

#[cfg(test)]
mod tests;
