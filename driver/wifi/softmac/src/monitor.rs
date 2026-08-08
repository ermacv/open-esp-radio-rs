//! Portable, non-retaining receive-monitor contract.
//!
//! A monitor sink observes a borrowed frame synchronously. It may copy the
//! frame into its own bounded storage, but it cannot retain the radio/DMA
//! borrow. Queue saturation is an ordinary observation loss and must never
//! backpressure the primary radio owner.

use crate::{
    MacRxMetadata,
    interface::{ChannelContextId, MonitorTapPoint},
};

/// One frame observed at an explicitly identified receive-pipeline boundary.
#[derive(Clone, Copy, Debug)]
pub struct MonitorFrame<'frame, Rate> {
    pub tap: MonitorTapPoint,
    pub channel_context: ChannelContextId,
    /// Bytes visible at this tap. They never include a hardware-stripped FCS.
    pub bytes: &'frame [u8],
    pub metadata: MacRxMetadata<Rate>,
    /// Complete logical MPDU length inferred from the receive status.
    ///
    /// `bytes` may be shorter when hardware consumed an authenticated cipher
    /// trailer before publishing the DMA view. The distinction prevents a
    /// capture consumer from treating such a frame as ordinary queue loss.
    pub logical_length: usize,
}

impl<Rate> MonitorFrame<'_, Rate> {
    pub const fn is_complete(&self) -> bool {
        self.bytes.len() == self.logical_length
    }
}

/// Why a best-effort monitor sink did not retain an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorDropReason {
    /// Every caller-owned capture slot was occupied.
    Full,
    /// The frame exceeds the sink's configured capture capacity.
    TooLong,
    /// Caller policy intentionally excluded this observation.
    Filtered,
}

/// Immediate result of a non-blocking monitor publication attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorPublishOutcome {
    Published,
    Dropped(MonitorDropReason),
}

/// Best-effort observation endpoint for one receive tap.
///
/// Implementations must return without waiting for capacity. An async adapter
/// can wake a consumer after copying into bounded storage, but the radio owner
/// never awaits that consumer and continues recycling its RX descriptor.
pub trait MonitorSink<Rate> {
    fn try_publish(&mut self, frame: MonitorFrame<'_, Rate>) -> MonitorPublishOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MacRxMetadata;

    #[test]
    fn capture_completeness_is_distinct_from_sink_overflow() {
        let bytes = [0_u8; 24];
        let complete = MonitorFrame::<()> {
            tap: MonitorTapPoint::Normalized,
            channel_context: ChannelContextId::PRIMARY,
            bytes: &bytes,
            metadata: MacRxMetadata::unavailable(),
            logical_length: bytes.len(),
        };
        let hardware_consumed_trailer = MonitorFrame {
            logical_length: bytes.len() + 8,
            ..complete
        };

        assert!(complete.is_complete());
        assert!(!hardware_consumed_trailer.is_complete());
        assert_ne!(
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full),
            MonitorPublishOutcome::Published
        );
    }
}
