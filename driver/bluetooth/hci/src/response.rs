//! Complete Controller responses retained by an operational HCI runner.

use bt_hci::PacketKind;

use crate::{BootstrapCommandCompleteEvent, LeDtmCommandCompleteEvent};

/// One complete Controller packet ready for publication toward the Host.
///
/// The trait is intentionally independent of command dispatch. A hardware
/// runner may retain an accepted command through arbitrary affine radio states,
/// build its response only at the proven completion boundary, and retry this
/// immutable packet across bounded output backpressure.
pub trait HciControllerResponse {
    /// HCI packet class published toward the Host.
    fn kind(&self) -> PacketKind;

    /// Complete packet body without an H4 indicator.
    fn as_bytes(&self) -> &[u8];
}

impl HciControllerResponse for BootstrapCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Closed Command Complete response set for the initial LE Controller.
///
/// Bootstrap and DTM responses have different storage types but share the
/// same publication boundary. Keeping the distinction typed avoids copying
/// either response into an unvalidated byte scratch buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeControllerCommandComplete {
    /// Pure software bootstrap command completion.
    Bootstrap(BootstrapCommandCompleteEvent),
    /// Completion supplied by the hardware-owned DTM session.
    Dtm(LeDtmCommandCompleteEvent),
}

impl HciControllerResponse for LeControllerCommandComplete {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Bootstrap(response) => response.as_bytes(),
            Self::Dtm(response) => response.as_bytes(),
        }
    }
}

impl From<BootstrapCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: BootstrapCommandCompleteEvent) -> Self {
        Self::Bootstrap(response)
    }
}

impl From<LeDtmCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: LeDtmCommandCompleteEvent) -> Self {
        Self::Dtm(response)
    }
}
